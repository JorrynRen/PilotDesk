/**
 * 数学计算 示例插件（工作流节点 + 测试面板）
 *
 * 用法：
 * - 工作流：新增节点 → 选「插件」→ 选「数学计算」→ 选具体命令。
 *   参数表单由 manifest 的 `input` 模式自动生成，**不需要写 React 代码**。
 * - 面板：右侧面板「插件」标签页 → 「数学计算 · 测试」。
 *   面板与工作流调用的是**同一批 handler**，所以面板里能算对、工作流里就能算对。
 *
 * 两条设计约定：
 * 1. **命令返回裸值**（数字），不是 `{ result: ... }` 这种包装对象 ——
 *    这样下游节点直接引用即可，不必多做一层字段提取。
 * 2. 工作流参数里可能带 `{{变量}}` 模板：模板没被上游替换时**必须报错**，
 *    不能静默算成 NaN（那会让用户拿到一个看起来"成功"的错误结果）。
 *
 * 三条必须遵守的插件约定（见插件开发 skill）：
 * - 入口是纯 JS：`new Function` 执行，不能用 import / JSX / TypeScript；UI 用 React.createElement
 * - `window` / `document` / `fetch` / `localStorage` 等全局被遮蔽为 undefined，
 *   跨生命周期共享状态用**模块作用域变量**（见下面的 pluginApi）
 */

// ── 数值解析 ───────────────────────────────────────────

/** 把工作流传来的值转成数字。模板未替换 / 空值 / 非数字一律抛错（错误由平台回显给用户） */
function toNumber(raw, label) {
  if (raw === undefined || raw === null || String(raw).trim() === '') {
    throw new Error(label + ' 不能为空');
  }
  var s = String(raw).trim();
  // 含 {{...}} 说明模板没被替换（上游节点没产出该字段），报错比算成 NaN 有用
  if (s.indexOf('{{') !== -1) {
    throw new Error(label + ' 仍是未替换的模板变量「' + s + '」：请检查上游节点是否输出了该字段');
  }
  var n = Number(s);
  if (!isFinite(n)) {
    throw new Error(label + ' 不是有效数字：「' + s + '」');
  }
  return n;
}

/** 取整数（阶乘、gcd/lcm 这类只能接受整数） */
function toInteger(raw, label) {
  var n = toNumber(raw, label);
  if (Math.floor(n) !== n) {
    throw new Error(label + ' 必须是整数（当前 ' + n + '）');
  }
  return n;
}

/** 把「逗号/空格/换行分隔」的文本（或数组）解析成数字列表 */
function toNumberList(raw, label) {
  var parts;
  if (Array.isArray(raw)) {
    parts = raw;
  } else {
    if (raw === undefined || raw === null || String(raw).trim() === '') {
      throw new Error(label + ' 不能为空');
    }
    var s = String(raw).trim();
    if (s.indexOf('{{') !== -1) {
      throw new Error(label + ' 仍是未替换的模板变量「' + s + '」：请检查上游节点是否输出了该字段');
    }
    parts = s.split(/[\s,，;；]+/).filter(function (x) { return x !== ''; });
  }
  if (parts.length === 0) {
    throw new Error(label + ' 里没有解析到任何数字');
  }
  return parts.map(function (x, i) { return toNumber(x, label + ' 第 ' + (i + 1) + ' 项'); });
}

/** 保留 digits 位小数，顺手抹掉浮点毛刺（0.1 + 0.2 = 0.30000000000000004） */
function roundTo(n, digits) {
  var f = Math.pow(10, digits);
  return Math.round(n * f) / f;
}

/** 结果必须是有限数：1/0、ln(0)、pow 溢出等都要当错误报出来 */
function ensureFinite(n, what) {
  if (!isFinite(n)) {
    throw new Error('计算结果不是有限数（' + what + '）：请检查输入是否越界');
  }
  return n;
}

// ── 运算表（面板的选项也取自这里，保证面板与工作流一致）──

// label 与 manifest 的 enumLabels 保持一致：值（中文）——
// 值会原样进工作流定义、handler 按它分支，中文只出现在界面与报错里。
var BINARY_OPS = {
  add: { label: 'add（相加）', fn: function (a, b) { return a + b; } },
  sub: { label: 'sub（相减）', fn: function (a, b) { return a - b; } },
  mul: { label: 'mul（相乘）', fn: function (a, b) { return a * b; } },
  div: { label: 'div（相除）', fn: function (a, b) { return a / b; } },
  mod: { label: 'mod（取余）', fn: function (a, b) { return a % b; } },
  pow: { label: 'pow（求幂）', fn: function (a, b) { return Math.pow(a, b); } },
};

var UNARY_OPS = {
  abs: { label: 'abs（绝对值）', fn: Math.abs },
  neg: { label: 'neg（取负）', fn: function (x) { return -x; } },
  round: { label: 'round（四舍五入）', fn: Math.round },
  floor: { label: 'floor（向下取整）', fn: Math.floor },
  ceil: { label: 'ceil（向上取整）', fn: Math.ceil },
  sqrt: { label: 'sqrt（平方根）', fn: Math.sqrt },
  ln: { label: 'ln（自然对数）', fn: Math.log },
  log10: { label: 'log10（常用对数）', fn: Math.log10 },
  exp: { label: 'exp（指数 e^x）', fn: Math.exp },
};

var AGG_OPS = {
  sum: { label: 'sum（求和）', fn: function (xs) { return xs.reduce(function (a, b) { return a + b; }, 0); } },
  avg: { label: 'avg（平均值）', fn: function (xs) { return xs.reduce(function (a, b) { return a + b; }, 0) / xs.length; } },
  min: { label: 'min（最小值）', fn: function (xs) { return Math.min.apply(null, xs); } },
  max: { label: 'max（最大值）', fn: function (xs) { return Math.max.apply(null, xs); } },
  median: {
    label: 'median（中位数）',
    fn: function (xs) {
      var ys = xs.slice().sort(function (a, b) { return a - b; });
      var mid = Math.floor(ys.length / 2);
      return ys.length % 2 === 1 ? ys[mid] : (ys[mid - 1] + ys[mid]) / 2;
    },
  },
  count: { label: 'count（个数）', fn: function (xs) { return xs.length; } },
  product: { label: 'product（求积）', fn: function (xs) { return xs.reduce(function (a, b) { return a * b; }, 1); } },
};

/** 三角函数：前三个是"角度 → 比值"，后三个是"比值 → 角度" */
var TRIG_OPS = {
  sin: { label: 'sin（正弦）', kind: 'forward', fn: Math.sin },
  cos: { label: 'cos（余弦）', kind: 'forward', fn: Math.cos },
  tan: { label: 'tan（正切）', kind: 'forward', fn: Math.tan },
  asin: { label: 'asin（反正弦）', kind: 'inverse', fn: Math.asin },
  acos: { label: 'acos（反余弦）', kind: 'inverse', fn: Math.acos },
  atan: { label: 'atan（反正切）', kind: 'inverse', fn: Math.atan },
};

/** 角度单位：forward 解释输入，inverse 解释输出 */
var ANGLE_UNITS = { rad: 'rad（弧度）', deg: 'deg（角度）' };

/** 两个整数的最大公约数 / 最小公倍数 */
var GCD_OPS = {
  gcd: { label: 'gcd（最大公约数）' },
  lcm: { label: 'lcm（最小公倍数）' },
};

/** 取枚举值：不在表里就报错并列出可选项，别让它落进一个 undefined 分支 */
function pickOp(table, raw, label, fallback) {
  var op = String(raw === undefined || raw === null || raw === '' ? fallback : raw).trim();
  var spec = table[op];
  if (!spec) {
    throw new Error('未知的' + label + '「' + op + '」：可选 ' + Object.keys(table).join(' / '));
  }
  return { op: op, spec: spec };
}

// ── 各命令的实现（面板与工作流共用；一律返回裸值）──────

function calcBinary(params) {
  var a = toNumber(params.a, 'a');
  var b = toNumber(params.b, 'b');
  var picked = pickOp(BINARY_OPS, params.op, '运算', 'add');
  if ((picked.op === 'div' || picked.op === 'mod') && b === 0) {
    throw new Error(picked.op === 'div' ? '除数不能为 0' : '取余的除数不能为 0');
  }
  return roundTo(ensureFinite(picked.spec.fn(a, b), picked.spec.label), 10);
}

function calcUnary(params) {
  var x = toNumber(params.value, 'value');
  var picked = pickOp(UNARY_OPS, params.op, '运算', 'abs');
  if ((picked.op === 'sqrt' || picked.op === 'ln' || picked.op === 'log10') && x <= 0) {
    throw new Error(picked.spec.label + ' 要求输入大于 0（当前 ' + x + '）');
  }
  return roundTo(ensureFinite(picked.spec.fn(x), picked.spec.label), 10);
}

function calcAggregate(params) {
  var xs = toNumberList(params.numbers, 'numbers');
  var picked = pickOp(AGG_OPS, params.op, '统计方式', 'sum');
  if (picked.op === 'product' && xs.length > 200) {
    throw new Error('求积的项数过多（' + xs.length + ' 项），容易溢出：请改小数据量或用求和');
  }
  return roundTo(ensureFinite(picked.spec.fn(xs), picked.spec.label), 10);
}

function calcRound(params) {
  var x = toNumber(params.value, 'value');
  var rawDigits = params.digits === undefined || params.digits === '' ? 2 : params.digits;
  var digits = toInteger(rawDigits, 'digits');
  if (digits < 0 || digits > 15) {
    throw new Error('digits 需在 0 ~ 15 之间（当前 ' + digits + '）');
  }
  return roundTo(x, digits);
}

function calcPercent(params) {
  var value = toNumber(params.value, 'value');
  var total = toNumber(params.total, 'total');
  if (total === 0) {
    throw new Error('total 不能为 0（无法计算占比）');
  }
  // 返回百分数本身（如 74 表示 74%）；要小数占比就再除以 100
  return roundTo((value / total) * 100, 6);
}

function calcTrig(params) {
  var picked = pickOp(TRIG_OPS, params.op, '三角函数', 'sin');
  var unit = String(params.unit === undefined || params.unit === '' ? 'rad' : params.unit).trim();
  if (!ANGLE_UNITS[unit]) {
    throw new Error('未知的角度单位「' + unit + '」：可选 ' + Object.keys(ANGLE_UNITS).join(' / '));
  }
  var v = toNumber(params.value, 'value');

  if (picked.spec.kind === 'forward') {
    // 输入是角度：deg 时先换成弧度
    var rad = unit === 'deg' ? (v * Math.PI) / 180 : v;
    if (picked.op === 'tan') {
      // Math.tan(pi/2) 不会返回 Infinity，而是一个巨大的浮点数 —— 直接拦掉更诚实
      var deg = unit === 'deg' ? v : (v * 180) / Math.PI;
      if (Math.abs((((deg % 180) + 180) % 180) - 90) < 1e-9) {
        throw new Error('tan 在 90° + k·180° 处无定义');
      }
    }
    return roundTo(ensureFinite(picked.spec.fn(rad), picked.spec.label), 10);
  }

  // 反三角：输入是比值，输出是角度
  if ((picked.op === 'asin' || picked.op === 'acos') && (v < -1 || v > 1)) {
    throw new Error(picked.spec.label + ' 要求输入在 -1 ~ 1 之间（当前 ' + v + '）');
  }
  var outRad = picked.spec.fn(v);
  var out = unit === 'deg' ? (outRad * 180) / Math.PI : outRad;
  return roundTo(ensureFinite(out, picked.spec.label), 10);
}

function calcFactorial(params) {
  var n = toInteger(params.n, 'n');
  if (n < 0) {
    throw new Error('n 不能为负数（当前 ' + n + '）');
  }
  if (n > 170) {
    throw new Error('n 最大支持 170（170! 已接近双精度上限，再大就没有意义了）');
  }
  var acc = 1;
  for (var i = 2; i <= n; i++) {
    acc *= i;
  }
  return acc;
}

/** 两个整数的 gcd / lcm（都取绝对值，符号不影响结果） */
function calcGcdLcm(params) {
  var a = Math.abs(toInteger(params.a, 'a'));
  var b = Math.abs(toInteger(params.b, 'b'));
  var picked = pickOp(GCD_OPS, params.op, '运算', 'gcd');

  if (a === 0 && b === 0) {
    throw new Error('a 与 b 同时为 0 时无法计算');
  }
  // 辗转相除
  var x = a;
  var y = b;
  while (y !== 0) {
    var t = x % y;
    x = y;
    y = t;
  }
  var gcd = x;
  if (picked.op === 'gcd') return gcd;
  if (gcd === 0) {
    return 0;
  }
  var lcm = (a / gcd) * b; // 先除再乘，减小溢出风险
  return ensureFinite(lcm, 'lcm');
}

/** 把 value 夹到 [min, max] 区间内 */
function calcClamp(params) {
  var v = toNumber(params.value, 'value');
  var lo = toNumber(params.min, 'min');
  var hi = toNumber(params.max, 'max');
  if (lo > hi) {
    throw new Error('min 不能大于 max（min=' + lo + ', max=' + hi + '）');
  }
  return Math.min(Math.max(v, lo), hi);
}

// 命令 id → 实现（register 与面板都从这里取，避免两处对不上）
var COMMANDS = {
  'math.binary': { title: '二元运算', fn: calcBinary },
  'math.unary': { title: '一元运算', fn: calcUnary },
  'math.aggregate': { title: '聚合统计', fn: calcAggregate },
  'math.round': { title: '按位四舍五入', fn: calcRound },
  'math.percentage': { title: '百分比', fn: calcPercent },
  'math.trigonometry': { title: '三角函数', fn: calcTrig },
  'math.factorial': { title: '阶乘', fn: calcFactorial },
  'math.gcd_lcm': { title: '最大公约数 / 最小公倍数', fn: calcGcdLcm },
  'math.clamp': { title: '区间限制', fn: calcClamp },
};

// ── 测试面板 ──────────────────────────────────────────

/** onLoad 里赋值。面板组件拿不到 api，用模块作用域变量传进去 */
var pluginApi = null;

/** 枚举下拉的选项（从运算表派生，保证与 handler 接受的值一致） */
function optionsOf(table) {
  return Object.keys(table).map(function (k) {
    return { value: k, label: table[k].label };
  });
}

function unitOptions() {
  return Object.keys(ANGLE_UNITS).map(function (k) {
    return { value: k, label: ANGLE_UNITS[k] };
  });
}

/** 面板里每个命令的试算表单字段（key 必须与 manifest 的 input 属性名一致） */
var PANEL_FORMS = [
  {
    id: 'math.binary',
    fields: [
      { key: 'a', label: 'a', def: '12' },
      { key: 'op', label: '运算', def: 'add', options: optionsOf(BINARY_OPS) },
      { key: 'b', label: 'b', def: '8' },
    ],
  },
  {
    id: 'math.unary',
    fields: [
      { key: 'value', label: 'value', def: '16' },
      { key: 'op', label: '运算', def: 'sqrt', options: optionsOf(UNARY_OPS) },
    ],
  },
  {
    id: 'math.aggregate',
    fields: [
      { key: 'numbers', label: 'numbers（逗号/空格/换行分隔）', def: '3, 1, 4, 1, 5, 9, 2, 6' },
      { key: 'op', label: '统计方式', def: 'avg', options: optionsOf(AGG_OPS) },
    ],
  },
  {
    id: 'math.round',
    fields: [
      { key: 'value', label: 'value', def: '3.1415926' },
      { key: 'digits', label: 'digits（0~15）', def: '2' },
    ],
  },
  {
    id: 'math.percentage',
    fields: [
      { key: 'value', label: 'value（部分）', def: '37' },
      { key: 'total', label: 'total（总量）', def: '50' },
    ],
  },
  {
    id: 'math.trigonometry',
    fields: [
      { key: 'op', label: '三角函数', def: 'sin', options: optionsOf(TRIG_OPS) },
      { key: 'value', label: 'value（sin/cos/tan 填角度；asin/acos/atan 填比值）', def: '30' },
      { key: 'unit', label: '角度单位', def: 'deg', options: unitOptions() },
    ],
  },
  {
    id: 'math.factorial',
    fields: [{ key: 'n', label: 'n（0~170 的整数）', def: '5' }],
  },
  {
    id: 'math.gcd_lcm',
    fields: [
      { key: 'a', label: 'a（整数）', def: '12' },
      { key: 'op', label: '运算', def: 'gcd', options: optionsOf(GCD_OPS) },
      { key: 'b', label: 'b（整数）', def: '18' },
    ],
  },
  {
    id: 'math.clamp',
    fields: [
      { key: 'value', label: 'value', def: '150' },
      { key: 'min', label: 'min', def: '0' },
      { key: 'max', label: 'max', def: '100' },
    ],
  },
];

var S = {
  label: { fontSize: 'var(--fs-11)', color: 'var(--text-secondary)', display: 'block', marginBottom: 2 },
  input: {
    width: '100%', padding: '5px 8px', borderRadius: 'var(--radius-md)',
    border: '1px solid var(--border)', backgroundColor: 'var(--bg-primary)',
    color: 'var(--text-primary)', fontSize: 'var(--fs-12)', outline: 'none',
  },
  row: { marginBottom: 8 },
  btn: {
    padding: '5px 12px', borderRadius: 'var(--radius-md)', border: 'none',
    backgroundColor: 'var(--accent)', color: '#fff', fontSize: 'var(--fs-12)', cursor: 'pointer',
  },
  out: {
    marginTop: 10, padding: '8px 10px', borderRadius: 'var(--radius-md)',
    backgroundColor: 'var(--bg-tertiary)', fontSize: 'var(--fs-12)', whiteSpace: 'pre-wrap',
    wordBreak: 'break-all', fontFamily: 'monospace',
  },
};

/** 命令返回的是裸值：数字/字符串直接显示，对象才做 JSON 化（面板不做额外提取） */
function fmt(value) {
  if (value === null || value === undefined) return '（无返回值）';
  return typeof value === 'object' ? JSON.stringify(value, null, 2) : String(value);
}

function MathTestPanel() {
  var cmdState = React.useState('math.binary');
  var cmdId = cmdState[0];
  var setCmdId = cmdState[1];

  // 所有命令的值都放在一个对象里（键为 命令id.字段名），切命令时不必重置 ——
  // 没填过的字段回落到表单默认值即可，省掉一个"effect 里 setState"的坑
  var valsState = React.useState({});
  var vals = valsState[0];
  var setVals = valsState[1];

  var outState = React.useState(null);
  var out = outState[0];
  var setOut = outState[1];

  var form = PANEL_FORMS.filter(function (f) { return f.id === cmdId; })[0] || PANEL_FORMS[0];

  function valueOf(field) {
    var k = form.id + '.' + field.key;
    return Object.prototype.hasOwnProperty.call(vals, k) ? vals[k] : field.def;
  }

  function setValue(field, v) {
    var k = form.id + '.' + field.key;
    var next = {};
    for (var kk in vals) { if (Object.prototype.hasOwnProperty.call(vals, kk)) next[kk] = vals[kk]; }
    next[k] = v;
    setVals(next);
  }

  function run() {
    if (!pluginApi) {
      setOut({ ok: false, text: '插件尚未加载完成，稍后再试' });
      return;
    }
    var params = {};
    form.fields.forEach(function (f) { params[f.key] = valueOf(f); });
    setOut({ ok: true, text: '计算中…' });
    // 走的是与工作流完全相同的命令通道：面板能过，工作流就能过
    pluginApi.commands.execute(form.id, params).then(function (r) {
      setOut({
        ok: r && r.success,
        text: r && r.success ? fmt(r.data) : String((r && r.error) || '未知错误'),
      });
    }).catch(function (e) {
      setOut({ ok: false, text: String(e && e.message ? e.message : e) });
    });
  }

  function fieldNode(f) {
    var v = valueOf(f);
    var common = { value: v, onChange: function (ev) { setValue(f, ev.target.value); }, style: S.input };
    var control = f.options
      ? React.createElement('select', common, f.options.map(function (o) {
          return React.createElement('option', { key: o.value, value: o.value }, o.label);
        }))
      : React.createElement('input', Object.assign({ type: 'text' }, common));
    return React.createElement('div', { key: f.key, style: S.row },
      React.createElement('label', { style: S.label }, f.label),
      control,
    );
  }

  return React.createElement('div', { style: { padding: 10 } },
    React.createElement('div', { style: S.row },
      React.createElement('label', { style: S.label }, '试算命令（与工作流用的是同一批 handler）'),
      React.createElement('select', {
        value: form.id,
        onChange: function (ev) { setCmdId(ev.target.value); setOut(null); },
        style: S.input,
      }, PANEL_FORMS.map(function (f) {
        var meta = COMMANDS[f.id];
        return React.createElement('option', { key: f.id, value: f.id }, meta ? meta.title : f.id);
      })),
    ),
    form.fields.map(fieldNode),
    React.createElement('button', { onClick: run, style: S.btn }, '计算'),
    out ? React.createElement('div', {
      style: Object.assign({}, S.out, { color: out.ok ? 'var(--text-primary)' : 'var(--danger)' }),
    }, out.text) : null,
  );
}

// ── 插件入口 ──────────────────────────────────────────

export default {
  onLoad: function (api) {
    console.log('[MathCalc] Plugin loaded');
    pluginApi = api;

    // 注册面板（覆盖 manifest 里声明的默认占位面板）
    api.ui.addPanel({
      id: 'math-test-panel',
      title: '数学计算 · 测试',
      component: MathTestPanel,
    });

    // 注册命令：**manifest 的 contributes.commands 只是声明**，
    // 只声明不注册的话，工作流调用时会返回「命令未注册」。
    Object.keys(COMMANDS).forEach(function (id) {
      api.commands.register(id, async function (params) {
        return COMMANDS[id].fn(params || {});
      });
    });

    console.log('[MathCalc] 已注册命令：' + Object.keys(COMMANDS).join(', '));
  },

  onUnload: function () {
    pluginApi = null;
    console.log('[MathCalc] Plugin unloaded');
  },
};
