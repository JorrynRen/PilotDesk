/**
 * KnowledgeGraph — 知识图谱（WebGL / Three.js）
 *
 * **为什么不是 SVG**：旧版用 SVG 画节点与边，实测天花板就在"每帧要不要重画几何"这条线上 ——
 * 静态 800 个图元还有 57 FPS，但只要每帧改一次整组的 `transform`，390 个节点就掉到 8 FPS
 * （Chromium 对 SVG 内容是 **CPU 光栅化**）。目标规模是上万级节点，那条曲线没有优化空间。
 * 换到 WebGL 后实测 **2 万节点 + 7 万边：静态 / 相机自转 / 拖动环绕 都是 59.9 FPS、0% 掉帧**。
 *
 * 三件事撑住规模：
 *   ① 节点 = 一次 `drawBuffers`（`THREE.Points`，逐点带颜色与世界半径）；
 *      边 = 一次 `drawArrays`（`THREE.LineSegments`，逐顶点颜色）；
 *   ② **按需重绘**：不脏就一帧都不提交（实测静态 6 秒 GL 提交 0 次）；
 *   ③ 分块**只展开当前聚焦的那一个文件**，规模闸门交给「知识」列表的筛选。
 *
 * 节点是**不透明的球**（见 POINT_FRAG）：旧版给圆点留了半透明边缘，结果背后的连线会
 * "从节点里穿出来" —— 半透明节点在混合通道里画，深度写入与遮挡都不干净。
 * 现在整颗球不透明、照常写深度，连线一律被正确挡住；立体感靠球面法线做明暗与高光。
 *
 * **两档，不是三档**（文件 → 分块）。曾设计过"缩略时折成簇"的 L0，已删除：
 * 折叠轴不可靠（库可能没有专属属性、属性类型不定、标签多值不能折叠），
 * 详见 graphModel.ts 的文件头。
 *
 * 标签一律**按需**（悬停 / 选中），且是**绝对定位的 HTML**：上万节点不可能有上万个文本对象。
 */

import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import * as THREE from 'three';
import { OrbitControls } from 'three/examples/jsm/controls/OrbitControls.js';
import { Crosshair, Eye, Layers, Maximize2, Palette, RotateCcw, Share2, SlidersHorizontal, Sparkles, Trash2, type LucideIcon } from 'lucide-react';
import {
  filterEntries,
  fileNodeId,
  hasActiveEntryFilters,
  useKnowledgeStore,
} from '../../stores/knowledgeStore';
import {
  buildCore,
  buildScene,
  type GraphScene,
  type SceneNode,
} from './graphModel';
import {
  CONTAINS_COLOR,
  CONTAINS_LABEL,
  EDGE_COLOR,
  EDGE_LABEL,
  ORIGIN_COLOR,
  ORIGIN_LABEL,
  entryTitle,
  type KnowledgeBase,
  type KnowledgeEdgeKind,
  type KnowledgeEntry,
  type KnowledgeGraphNode,
  type KnowledgeOrigin,
} from '../../types/knowledge';

/**
 * 面板里一个内容块的标题：图标 + 文本（+ 右侧附注）。
 *
 * 加图标是因为这一栏的类目已经不少了，光靠 10px 的灰字标题扫读太慢 ——
 * 图标是"定位"用的锚点，比文字先被认出来。右侧附注必须是**可选**的：
 * 有的块没有附注、有的是动态值（数量 / 系统状态），塞进一个组件比每块各写一遍标题省事。
 *
 * **图标必须着色（主题色）**：跟标题文字一样灰的话，10px 的灰线在灰底上几乎认不出形状，
 * 等于白加一个图标（用户明确反馈过"非有色图标视觉效果差"）。
 * 文本仍保持弱化色，让"彩色锚点 + 安静文字"形成层次。这也与房子里的既有做法一致
 * （知识库列表头与目录行的小图标都是 `var(--accent)`）。
 */
function BlockTitle({ icon: Icon, text, right }: { icon: LucideIcon; text: string; right?: ReactNode }) {
  return (
    <div className="flex items-center justify-between mb-1.5">
      <span className="flex items-center gap-1 text-[10px]" style={{ color: 'var(--text-tertiary)' }}>
        <Icon size={11} className="shrink-0" style={{ color: 'var(--accent)' }} />
        {text}
      </span>
      {right !== undefined && (
        <span className="text-[10px] tabular-nums" style={{ color: 'var(--text-tertiary)' }}>{right}</span>
      )}
    </div>
  );
}

/** 边的类型（含结构边「分块归属」）：图例与过滤都按这一个联合类型走 */
type LinkKind = KnowledgeEdgeKind | 'contains';

const LEGEND: LinkKind[] = ['same_field', 'shared_tag', 'attachment', 'supersedes', 'contains'];
const kindLabel = (k: LinkKind) => (k === 'contains' ? CONTAINS_LABEL : EDGE_LABEL[k]);
const kindColor = (k: LinkKind) => (k === 'contains' ? CONTAINS_COLOR : EDGE_COLOR[k]);
const kindHex = (k: LinkKind) => parseInt(kindColor(k).slice(1), 16);

/**
 * **默认把分块全展开的规模上限**：文件数 + 全部分块数 ≤ 这个值就自动展开所有文件的分块。
 *
 * 为什么默认展开而不是默认折叠：**大部分用户、大部分知识库都是小规模的**，
 * 而默认折叠会让图谱看起来空荡荡一片点、完全体现不出"知识之间有关系"。
 * 只有到了这个规模才退回文件级 —— 那时再展开就真的是糊成一团了，而且逐个文件拉分块也慢。
 * 想看得更多/更少，用「知识」列表的筛选，或用左侧的「展开全部分块 / 收起全部分块」。
 */
const AUTO_EXPAND_MAX_NODES = 2000;
/** 初始相机距离的下限/上限：太近会怼在图上，太远小库会变成一个小点 */
const CAM_MIN_DIST = 18;
const CAM_MAX_DIST = 1500;
/** 相机自转的角速度（度/秒）—— **默认值**，用户可在左栏拉动条里改 */
const AUTO_ROTATE_DEG_PER_SEC = 3;
/**
 * 相机"手抖"幅度（占相机距离的比例）：三个不同频率的正弦叠加 ⇒ 不循环、像手持悬浮。
 *
 * 为什么要在**相机**层面做而不是挪节点：挪节点会改布局（那是结构信息），
 * 而且每帧改几十万个坐标。相机漂移只是改一个视矩阵，O(1)，且不影响任何数据。
 * 注意它必须在 `controls.update()` **之后**加、下一帧再撤掉，否则会污染轨道状态。
 * 这是**默认值**，用户可在左栏拉动条里改。
 */
const CAM_DRIFT = 0.012;
/** 命中半径（屏幕像素）：给得比节点本身大，小点也要点得中 */
const PICK_RADIUS = 22;
/** 判定"这是拖动而不是点击"的位移阈值（屏幕像素） */
const DRAG_SLOP = 3;

/** 布局变化时的过渡时长（ms）：展开/收起分块、换库入场都用它 */
const TWEEN_MS = 520;
/**
 * 拖动松手后"弹性吸回原位"的弹簧参数。
 * 用 `ω`（角频率）与 `ζ`（阻尼比）而不是 k/c：这两个量直接决定手感 ——
 * `ζ = 0.45` 是欠阻尼，大约回弹两下就停，既有"弹"的感觉又不会晃个不停。
 */
const SPRING_OMEGA = 14;
const SPRING_ZETA = 0.45;
/** 相机补间时长（ms） */
const CAM_TWEEN_MS = 420;
/** 边的常态透明度（入场动画会从 0 补到它） */
const LINE_OPACITY = 0.4;

/** 缓出三次：起步快、收尾稳，比线性"更像有惯性" */
const easeOutCubic = (t: number) => 1 - (1 - t) ** 3;
const easeInOutCubic = (t: number) => (t < 0.5 ? 4 * t * t * t : 1 - (-2 * t + 2) ** 3 / 2);

/**
 * 拖动时"关系节点被吸过来"的强度与衰减半径。
 *
 * 权重按**静止时的间距**衰减：`w = ATTRACT / (1 + (d / ATTRACT_RADIUS)²)` ——
 * 紧邻的邻居几乎跟着走一半，隔得远的几乎不动。用平方衰减而不是线性：
 * 线性衰减会让"整张图都在微微动"，那种联动看着像故障而不是手感。
 * `ATTRACT` 是**默认值**（用户可在左栏拉动条里改），`ATTRACT_RADIUS` 固定。
 */
const ATTRACT = 0.45;
const ATTRACT_RADIUS = 32;
/** 选中描边的空隙与宽度（**屏幕像素**，不随节点大小与缩放变化） */
const RING_GAP_PX = 3;
const RING_STROKE_PX = 2;
/** 节点"眨眼"的角频率（rad/s）与亮度起伏幅度 —— 这两个是**默认值**，用户可在左栏拉动条里改 */
const TWINKLE_SPEED = 1.7;
const TWINKLE_AMP = 0.3;
/** 相位递进步长 = 黄金角：相邻节点的相位差得尽量"别扭"，闪烁才不成片同步 */
const PHASE_STRIDE = 2.399963229728653;

/* ────────────────────────── 可调参数（拉动条） ────────────────────────── */

/**
 * 图谱的动效与手感参数。
 *
 * **为什么把它们做成拉动条**：这几个数（3°/s、1.2%、1.7rad/s、30%、45%）是按"看得见但不吵"调的默认值，
 * 但"吵不吵"取决于屏幕大小、库的规模、以及坐在这台机器前面的人 —— 那是口味不是对错。
 * 让用户自己拧，比我们猜第二轮便宜得多。
 *
 * **持久化走 localStorage，不进数据库**：它是本机偏好而不是业务数据。进库会让"换台机器就该用默认值"
 * 变成一个要同步、要迁移的问题，而这个问题的价值是零。
 */
interface MotionParams {
  /** 相机环绕角速度（度/秒） */
  rotateSpeed: number;
  /** 相机"手抖"幅度（占相机距离的比例） */
  drift: number;
  /** 节点眨眼角频率（rad/s） */
  twinkleSpeed: number;
  /** 节点眨眼幅度（亮度起伏的比例） */
  twinkleAmp: number;
  /** 拖动时关系节点被吸过来的强度（0 = 不吸） */
  attract: number;
}

const MOTION_PARAMS_KEY = 'kb-graph-motion-params';
const MOTION_PARAMS_DEFAULT: MotionParams = {
  rotateSpeed: AUTO_ROTATE_DEG_PER_SEC,
  drift: CAM_DRIFT,
  twinkleSpeed: TWINKLE_SPEED,
  twinkleAmp: TWINKLE_AMP,
  attract: ATTRACT,
};

/**
 * 每个参数的取值范围、步长与**显示口径**。
 *
 * 收在一处是因为这三者必须一致：显示了 `1.20%` 而存的是 `0.012`，如果 UI 与 clamp 各写一份范围，
 * 迟早出现"滑到头了但显示还能再走"或者"存进去的值被读回时被悄悄改掉"。
 */
const PARAM_SPECS: {
  key: keyof MotionParams;
  label: string;
  min: number;
  max: number;
  step: number;
  fmt: (v: number) => string;
}[] = [
  { key: 'rotateSpeed', label: '环绕速度', min: 0, max: 12, step: 0.5, fmt: (v) => `${v.toFixed(1)}°/s` },
  { key: 'drift', label: '手抖幅度', min: 0, max: 0.05, step: 0.002, fmt: (v) => `${(v * 100).toFixed(1)}%` },
  { key: 'twinkleSpeed', label: '眨眼频率', min: 0.5, max: 6, step: 0.1, fmt: (v) => `${v.toFixed(1)}/s` },
  { key: 'twinkleAmp', label: '眨眼幅度', min: 0, max: 0.8, step: 0.05, fmt: (v) => `${Math.round(v * 100)}%` },
  { key: 'attract', label: '吸附强度', min: 0, max: 1, step: 0.05, fmt: (v) => `${Math.round(v * 100)}%` },
];

/**
 * 读参数：**逐字段**夹到范围内、缺的补默认值。
 *
 * 必须夹：localStorage 里一个手改的 `drift: 5`（= 5 倍相机距离）会把画面甩飞、`dt` 再大点就直接瞎了，
 * 而用户根本不会往"是我改坏了配置"那条路上想。**外部输入一律不信任，哪怕来源是自己的前端。**
 */
function readMotionParams(): MotionParams {
  const out = { ...MOTION_PARAMS_DEFAULT };
  try {
    const raw = localStorage.getItem(MOTION_PARAMS_KEY);
    if (!raw) return out;
    const parsed = JSON.parse(raw) as Partial<MotionParams>;
    for (const s of PARAM_SPECS) {
      const v = parsed[s.key];
      if (typeof v === 'number' && Number.isFinite(v)) out[s.key] = Math.min(s.max, Math.max(s.min, v));
    }
  } catch {
    // 坏 JSON / 隐私模式：回默认值，别让"读偏好失败"连带把图谱也打不开
  }
  return out;
}

function writeMotionParams(p: MotionParams) {
  try {
    localStorage.setItem(MOTION_PARAMS_KEY, JSON.stringify(p));
  } catch {
    // 写不进去就只在本会话生效 —— 不值得为它弹一个错
  }
}

/**
 * 让内容刚好铺满视口的相机距离。
 *
 * 按**外接球半径**算，而不是写死一个常数：十几个节点的库不该被摆在画布中心当一小撮。
 * 竖直半 FOV 是 `fov/2`（45° ⇒ 22.5°，tan ≈ 0.414），球半径 R 要装进去就需要 `R / tan`；
 * 再留 15% 余量给标签与边缘。
 */
function fitDistance(radius: number, fovDeg: number): number {
  const d = (radius * 1.15) / Math.tan((fovDeg * Math.PI) / 360);
  return Math.min(CAM_MAX_DIST, Math.max(CAM_MIN_DIST, d));
}

/** 命中测试的复用对象：批量投影时一个都不新分配 */
const _vp = new THREE.Matrix4();
const _mwi = new THREE.Matrix4();

/* ────────────────────────── 动效偏好 ────────────────────────── */

const MOTION_KEY = 'kb-graph-motion';
const TOOLTIP_KEY = 'kb-graph-tooltip';
type MotionPref = 'auto' | 'on' | 'off';
const MOTION_DEFAULT: MotionPref = 'on';
const MOTION_OPTIONS: { value: MotionPref; label: string }[] = [
  { value: 'auto', label: '跟随系统' },
  { value: 'on', label: '开' },
  { value: 'off', label: '关' },
];
function readMotionPref(): MotionPref {
  const v = localStorage.getItem(MOTION_KEY);
  return v === 'on' || v === 'off' || v === 'auto' ? v : MOTION_DEFAULT;
}

/* ────────────────────────── 着色器 ────────────────────────── */

/**
 * 节点：把圆点当成一个半球做明暗，做出「球」的立体感；**不透明**（见文件头）。
 * 另外每颗星有自己的相位，动效开着时各自缓慢明暗起伏 —— 这才是"动态星空"，
 * 背景那层星点动得再欢，节点不动就还是像贴上去的。
 */
const POINT_VERT = `
  attribute float aSize;
  attribute vec3 aColor;
  attribute float aPhase;
  uniform float uScale;
  uniform float uTime;
  uniform float uTwinkle;
  uniform float uTwinkleSpeed;
  uniform float uTwinkleAmp;
  varying vec3 vColor;
  varying float vPulse;
  void main() {
    vColor = aColor;
    float wave = sin(uTime * uTwinkleSpeed + aPhase);
    vPulse = 1.0 + uTwinkle * uTwinkleAmp * wave;
    vec4 mv = modelViewMatrix * vec4(position, 1.0);
    // 大小也跟着轻微起伏，否则只像"呼吸的灯"而不像"星"。
    // 尺寸幅度固定取亮度幅度的 1/3：一个滑块同时管两者，不为这点小事再加一个旋钮。
    gl_PointSize = clamp(
      aSize * (1.0 + uTwinkle * uTwinkleAmp * 0.33 * wave) * uScale / max(1.0, -mv.z),
      1.0, 128.0);
    gl_Position = projectionMatrix * mv;
  }
`;
const POINT_FRAG = `
  varying vec3 vColor;
  varying float vPulse;
  void main() {
    vec2 d = gl_PointCoord * 2.0 - 1.0;
    float r2 = dot(d, d);
    if (r2 > 1.0) discard;
    // 球面法线：中心 z=1、边缘 z=0
    float z = sqrt(max(0.0, 1.0 - r2));
    vec3 n = vec3(d, z);
    vec3 L = normalize(vec3(-0.45, 0.55, 0.70));
    float diff = max(dot(n, L), 0.0);
    float rim = pow(1.0 - z, 2.5);
    float spec = pow(max(dot(reflect(-L, n), vec3(0.0, 0.0, 1.0)), 0.0), 28.0);
    // 高光不跟着脉动（那是"灯"不是"星"）
    vec3 col = vColor * vPulse * (0.32 + 0.82 * diff) * (1.0 - 0.32 * rim) + vec3(spec) * 0.5;
    // 最外一圈给一点点 alpha 渐变做抗锯齿。**alpha 只是写进画布的通道**：
    // 材质是 opaque、开着深度写入，所以球内部严格遮挡后面的连线。
    gl_FragColor = vec4(col, smoothstep(1.0, 0.80, r2));
  }
`;

/**
 * 选中描边：**固定屏幕像素宽**的一圈（不是节点尺寸的倍数）。
 * 用倍数会让大节点拖一条粗边、小节点几乎看不见 —— 描边是"标记"而不是"形状的一部分"，
 * 标记的粗细不该跟着被标记的东西变。
 */
const RING_VERT = `
  uniform float uScale;
  uniform float uNodeWorld;
  uniform float uGapPx;
  uniform float uStrokePx;
  varying float vPx;
  void main() {
    vec4 mv = modelViewMatrix * vec4(position, 1.0);
    float pxPerWorld = uScale / max(1.0, -mv.z);
    gl_PointSize = clamp(uNodeWorld * pxPerWorld + 2.0 * uGapPx + uStrokePx, 4.0, 320.0);
    vPx = gl_PointSize;
    gl_Position = projectionMatrix * mv;
  }
`;
const RING_FRAG = `
  uniform vec3 uColor;
  uniform float uStrokePx;
  varying float vPx;
  void main() {
    vec2 d = gl_PointCoord * 2.0 - 1.0;
    float r = length(d);
    // 把"固定像素宽"换算成 [0,1] 的半径区间：环的外径是 vPx 像素
    float inner = 1.0 - 2.0 * uStrokePx / max(vPx, 1.0);
    if (r > 1.0 || r < inner) discard;
    gl_FragColor = vec4(uColor, 1.0);
  }
`;
/* ────────────────────────── Three.js 现场 ────────────────────────── */

interface GlCtx {
  renderer: THREE.WebGLRenderer;
  scene: THREE.Scene;
  camera: THREE.PerspectiveCamera;
  controls: OrbitControls;
  group: THREE.Group;
  points: THREE.Points | null;
  lines: THREE.LineSegments | null;
  /** 选中环（单个点） */
  ring: THREE.Points;
  ringPos: THREE.BufferAttribute;
  mat: THREE.ShaderMaterial;
  ringMat: THREE.ShaderMaterial;
  lineMat: THREE.LineBasicMaterial;
  /** 位置属性的当前值（含拖动覆盖）——上传与命中测试都读它 */
  worldPos: Float32Array;
  nodePos: THREE.BufferAttribute | null;
  linePos: THREE.BufferAttribute | null;
  /** 拖动后用增量上传，避免每次把整条边缓冲重传 */
  edgeCount: number;
  dirty: boolean;
  raf: number;
  /**
   * 图谱动效开着没有（= 左侧三档开关折出来的那个布尔）。
   * 它同时驱动三样：相机自转、节点"眨眼"、以及（CSS 那层）星空漂移。
   */
  motion: boolean;
  /** 屏幕像素/世界单位 的换算分子：h / (2·tan(fov/2)) */
  uScale: number;
  /**
   * 位置补间：把节点从 `from` 平滑移到 `to`（`to` 单独存一份，因为 `worldPos` 是插值写出的目标）。
   * 展开/收起分块、进图谱入场都走它 —— 否则整张图会瞬移，那正是最刺眼的一处。
   */
  tween: { from: Float32Array; to: Float32Array; start: number; dur: number; fadeEdges: boolean } | null;
  /** 相机补间（点节点 / 重置视角 / 自动取景）：`from*` → `to*` */
  camTween: {
    fromP: THREE.Vector3;
    toP: THREE.Vector3;
    fromT: THREE.Vector3;
    toT: THREE.Vector3;
    start: number;
    dur: number;
  } | null;
  /** 拖动松手后的弹性回位：**可能同时有多个**（被"吸"过来的邻居要一起弹回去） */
  springs: Map<number, { pos: THREE.Vector3; vel: THREE.Vector3; anchor: THREE.Vector3 }>;
  /**
   * 上一次**稳定**下来的节点位置（按节点 id）。
   * 换布局时按 id 取"从哪出发"：老节点从原位出发、新节点从宿主文件的位置长出来，
   * 于是展开分块看起来是"从文件里冒出来"而不是整张图重排。
   * `null` = 还没定过（入场）。
   */
  settled: Map<string, [number, number, number]> | null;
  /** `settled` 属于哪个库：换库要重新入场 */
  settledBase: string;
  /**
   * 用户有没有自己动过相机（缩放/环绕/平移）：动过之后就不再自动取景。
   *
   * **这类"属于一次 GL 现场"的状态必须放在 ctx 里，不能放组件 ref** ——
   * StrictMode 下 effect 是「挂载 → 卸载 → 再挂载」，GL 现场会被重建，而组件 ref **不会**。
   * 放组件 ref 的后果实测过：第二次挂载时"这个库已经取过景"的判断还留着，
   * 于是**一个静态的小库永远不取景**，内容在画布中心只有 40×38 像素（开发态可见）。
   */
  userMoved: boolean;
  /** 上一次自动取景时的内容半径（同一现场内跳过 10% 以内的微调） */
  fitRadius: number;
  /** 上一次自动取景属于哪个库 */
  fitBase: string;
}

interface DragState {
  index: number;
  moved: boolean;
  /** 按下时的屏幕坐标：用来区分"点一下"和"拖一下" */
  startX: number;
  startY: number;
  plane: THREE.Plane;
  grabOffset: THREE.Vector3;
  /** 被拖节点的静止位置：邻居的位移量按它算，松手时也回到它 */
  anchor: THREE.Vector3;
  /** 关系节点 → 吸过来的权重与它自己的静止位置（权重按两者的静止间距衰减） */
  attract: Map<number, { w: number; anchor: THREE.Vector3 }>;
}

export interface KnowledgeGraphProps {
  base: KnowledgeBase;
  entries: KnowledgeEntry[];
}

export function KnowledgeGraph({ base, entries }: KnowledgeGraphProps) {
  const storeFiles = useKnowledgeStore((s) => s.files);
  const fileRelations = useKnowledgeStore((s) => s.relations);
  const filters = useKnowledgeStore((s) => s.entryFilters);
  const graphChunks = useKnowledgeStore((s) => s.graphChunks);
  const expandGraphFiles = useKnowledgeStore((s) => s.expandGraphFiles);
  const collapseGraphFiles = useKnowledgeStore((s) => s.collapseGraphFiles);
  const loading = useKnowledgeStore((s) => s.loading);
  const setTab = useKnowledgeStore((s) => s.setTab);

  const [enabledKinds, setEnabledKinds] = useState<LinkKind[]>(LEGEND);
  const [selectedId, setSelectedId] = useState<string | null>(null);

  const [motionPref, setMotionPref] = useState<MotionPref>(readMotionPref);
  const [systemReduce, setSystemReduce] = useState(
    () => window.matchMedia('(prefers-reduced-motion: reduce)').matches,
  );
  const motionOn = motionPref === 'on' || (motionPref === 'auto' && !systemReduce);

  /* 可调参数：状态给 UI，`paramsRef` 给命令式路径（每帧的 tick 与拖动时的吸附都读它） */
  const [params, setParams] = useState<MotionParams>(readMotionParams);
  const paramsRef = useRef(params);
  useEffect(() => {
    paramsRef.current = params;
  }, [params]);

  const updateParam = (key: keyof MotionParams, value: number) => {
    // 用函数式更新：拖动条连发时，`{...paramsRef.current}` 可能落后一帧、把上一个滑块的改动带回去
    setParams((prev) => {
      const next = { ...prev, [key]: value };
      writeMotionParams(next);
      return next;
    });
  };
  const resetParams = () => {
    writeMotionParams(MOTION_PARAMS_DEFAULT);
    setParams(MOTION_PARAMS_DEFAULT);
  };
  const paramsDirty = PARAM_SPECS.some((s) => params[s.key] !== MOTION_PARAMS_DEFAULT[s.key]);

  useEffect(() => {
    const mq = window.matchMedia('(prefers-reduced-motion: reduce)');
    const onChange = () => setSystemReduce(mq.matches);
    mq.addEventListener('change', onChange);
    return () => mq.removeEventListener('change', onChange);
  }, []);

  const chooseMotion = (v: MotionPref) => {
    setMotionPref(v);
    localStorage.setItem(MOTION_KEY, v);
  };

  const hostRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const labelRef = useRef<HTMLDivElement>(null);
  const glRef = useRef<GlCtx | null>(null);
  const sceneRef = useRef<GraphScene | null>(null);
  const dragRef = useRef<DragState | null>(null);
  /** 悬停命中的节点索引（-1 = 没命中） */
  const hoverRef = useRef(-1);
  /** 选中节点的索引（-1 = 没选中）——命中测试与标签都读它，所以放 ref 而不是 state */
  const selectedIndexRef = useRef(-1);
  // 被拖动的节点**会钉住**（那是用户的显式意图），所以需要一张覆盖表 + 一个"全部归位"的出口
  const movedRef = useRef<Map<string, [number, number, number]>>(new Map());
  /** 「归位」按钮的可点状态：ref 不参与渲染，所以另存一份计数 */
  const [movedCount, setMovedCount] = useState(0);
  /** 清掉覆盖后，用它触发一次缓冲重传 */
  const [sceneRebuild, setSceneRebuild] = useState(0);
  /** 在空白处按下的位置：抬手时若没挪动就当作"点空白 = 取消选中" */
  const emptyDownRef = useRef<{ x: number; y: number } | null>(null);
  /** 标签当前指向的节点索引（-1 = 不显示）；由"悬停优先、否则选中"决定 */
  const labelIndexRef = useRef(-1);
  /** 每帧把标签钉在节点上（相机自转时标签必须跟着走） */
  const refreshLabelRef = useRef<() => void>(() => {});
  /**
   * 命中 / 移动 / 点击这三个是**命令式**操作，只被原生指针事件调用。
   * 通过 ref 转一手而不是直接闭包引用：GL 现场那个 effect 必须排在它们前面
   * （现场要先建好），直接引用会踩"变量在声明前被访问"。
   */
  const pickAtRef = useRef<(x: number, y: number) => number>(() => -1);
  const moveNodeRef = useRef<(index: number, x: number, y: number, z: number) => void>(() => {});
  const uploadNodesRef = useRef<(indices: number[]) => void>(() => {});
  const commitClickRef = useRef<(index: number) => void>(() => {});
  const syncRingRef = useRef<() => void>(() => {});

  const filtersActive = hasActiveEntryFilters(filters);

  const core = useMemo(
    () => buildCore(filterEntries(entries, filters), base, storeFiles, fileRelations, filtersActive),
    [entries, filters, base, storeFiles, fileRelations, filtersActive],
  );

  /** 已展开分块的文件：`文件节点 id` → 它的分块（已过筛选）。键的集合就是"哪些文件展开了" */
  const expanded = useMemo(() => {
    const m = new Map<string, KnowledgeEntry[]>();
    for (const [relPath, list] of Object.entries(graphChunks)) {
      m.set(fileNodeId(relPath), filterEntries(list, filters));
    }
    return m;
  }, [graphChunks, filters]);

  const scene = useMemo(
    () => buildScene({ core, expanded, enabledKinds }),
    [core, expanded, enabledKinds],
  );
  useEffect(() => {
    sceneRef.current = scene;
  }, [scene]);

  const nodeById = useMemo(() => new Map(core.nodes.map((n) => [n.id, n])), [core.nodes]);

  /** 能展开分块的文件（按分块从多到少）：自动展开与「展开全部分块」共用这一个口径 */
  const expandTargets = useMemo(
    () => core.nodes
      .filter((n) => n.kind === 'file' && n.chunkCount > 0 && n.sourceRef)
      .sort((a, b) => b.chunkCount - a.chunkCount)
      .map((n) => ({ relPath: n.sourceRef, chunkCount: n.chunkCount })),
    [core.nodes],
  );

  /**
   * 进图谱时**默认把分块全展开** —— 前提是规模小。
   *
   * 大部分用户、大部分知识库都是小规模的，默认折叠会让图谱看起来空荡荡一片点、
   * 完全体现不出"知识之间有关系"。只有 文件数 + 全部分块数 超过 `AUTO_EXPAND_MAX_NODES`
   * 才退回文件级（那时展开就真的糊了，而且逐个文件拉也慢）。
   *
   * **`loading` 必须参与前置判断**：换库时 `activeBaseId` 先变、`entries/files` 后到，
   * 只看"核心节点非空"会拿新库的 id 去展开上一个库的文件路径。
   * 每个库只做一次；用户手动收起后不会被再次打开。
   */
  const autoExpandedFor = useRef('');
  useEffect(() => {
    if (!base.id || loading || core.nodes.length === 0 || autoExpandedFor.current === base.id) return;
    autoExpandedFor.current = base.id;
    const total = core.nodes.length + expandTargets.reduce((s, t) => s + t.chunkCount, 0);
    if (total > AUTO_EXPAND_MAX_NODES) return;
    void expandGraphFiles(expandTargets, { silent: true });
  }, [base.id, loading, core.nodes.length, expandTargets, expandGraphFiles]);

  /* ── 初始化 GL 现场（只做一次） ── */
  useEffect(() => {
    const canvas = canvasRef.current;
    const host = hostRef.current;
    if (!canvas || !host) return;

    const renderer = new THREE.WebGLRenderer({ canvas, antialias: true, alpha: true });
    renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
    renderer.setClearColor(0x000000, 0); // 透明：让画布下方的星空层透上来

    const glScene = new THREE.Scene();
    const camera = new THREE.PerspectiveCamera(45, 1, 0.5, 4000);
    // 先给个中性距离：真正的取景由下面的"按内容外接球取景" effect 立刻接管
    camera.position.set(0, 0, 300);

    const controls = new OrbitControls(camera, canvas);
    controls.enableDamping = false; // 开阻尼就得每帧 update ⇒ 空闲不再为零
    controls.rotateSpeed = 0.8;
    controls.autoRotateSpeed = AUTO_ROTATE_DEG_PER_SEC / 6;

    const uScale = { value: 600 };
    const mat = new THREE.ShaderMaterial({
      uniforms: {
        uScale,
        uTime: { value: 0 },
        uTwinkle: { value: 0 },
        // 初值给默认，真正的值由"参数"那个 effect 立刻同步过来（它排在 init 之后）
        uTwinkleSpeed: { value: TWINKLE_SPEED },
        uTwinkleAmp: { value: TWINKLE_AMP },
      },
      vertexShader: POINT_VERT,
      fragmentShader: POINT_FRAG,
      transparent: false, // 不透明 ⇒ 深度写入与遮挡都干净（见文件头）
      depthWrite: true,
      depthTest: true,
    });
    const lineMat = new THREE.LineBasicMaterial({
      vertexColors: true,
      transparent: true,
      opacity: LINE_OPACITY,
      depthWrite: false, // 细线写深度会在节点上切出缝隙
      depthTest: true, // 但被节点挡住的边照旧不画
    });
    const ringMat = new THREE.ShaderMaterial({
      uniforms: {
        uScale,
        uNodeWorld: { value: 4 },
        uGapPx: { value: RING_GAP_PX },
        uStrokePx: { value: RING_STROKE_PX },
        uColor: { value: new THREE.Color(0x2563eb) },
      },
      vertexShader: RING_VERT,
      fragmentShader: RING_FRAG,
      transparent: false,
      depthWrite: false,
      depthTest: false, // 永远看得见：这是"选中反馈"，不该被别的节点挡住
    });
    const ringGeo = new THREE.BufferGeometry();
    const ringPos = new THREE.BufferAttribute(new Float32Array(3), 3);
    ringGeo.setAttribute('position', ringPos);
    const ring = new THREE.Points(ringGeo, ringMat);
    ring.frustumCulled = false;
    ring.renderOrder = 2;
    ring.visible = false;
    glScene.add(ring);

    const ctx: GlCtx = {
      renderer,
      scene: glScene,
      camera,
      controls,
      group: new THREE.Group(),
      points: null,
      lines: null,
      ring,
      ringPos,
      mat,
      ringMat,
      lineMat,
      worldPos: new Float32Array(0),
      nodePos: null,
      linePos: null,
      edgeCount: 0,
      dirty: true,
      raf: 0,
      motion: false,
      uScale: 600,
      tween: null,
      camTween: null,
      springs: new Map(),
      settled: null,
      settledBase: '',
      userMoved: false,
      fitRadius: 0,
      fitBase: '',
    };
    glScene.add(ctx.group);
    glRef.current = ctx;

    const markDirty = () => { ctx.dirty = true; };
    controls.addEventListener('change', markDirty);

    const resize = () => {
      const r = host.getBoundingClientRect();
      const w = Math.max(1, Math.round(r.width));
      const h = Math.max(1, Math.round(r.height));
      renderer.setSize(w, h, false);
      camera.aspect = w / h;
      camera.updateProjectionMatrix();
      // 世界半径 → 屏幕像素：h / (2·tan(fov/2))，与着色器里的换算必须一致
      ctx.uScale = h / (2 * Math.tan((camera.fov * Math.PI) / 360));
      uScale.value = ctx.uScale;
      markDirty();
    };
    resize();
    const ro = new ResizeObserver(resize);
    ro.observe(host);

    /* ── 指针：悬停拾取、选中、拖动节点。用**捕获阶段的原生监听** ──
     * OrbitControls 自己也在 canvas 上挂 pointerdown（非捕获），
     * 捕获阶段先跑，我们才能在它启动旋转之前把 `controls.enabled` 关掉。 */
    const placeLabel = (index: number) => {
      const el = labelRef.current;
      const sn = sceneRef.current;
      if (!el || !canvas || !sn || index < 0 || index >= sn.nodes.length) return;
      const v = new THREE.Vector3(
        ctx.worldPos[index * 3],
        ctx.worldPos[index * 3 + 1],
        ctx.worldPos[index * 3 + 2],
      ).project(camera);
      const rect = canvas.getBoundingClientRect();
      el.style.transform = `translate(${(v.x * 0.5 + 0.5) * rect.width + 12}px, ${(-v.y * 0.5 + 0.5) * rect.height + 10}px)`;
    };
    const showLabel = (index: number) => {
      const el = labelRef.current;
      const sn = sceneRef.current;
      if (!el || !sn || index < 0 || index >= sn.nodes.length) {
        labelIndexRef.current = -1;
        if (el) el.style.display = 'none';
        if (canvas) canvas.style.cursor = 'grab';
        return;
      }
      const node = sn.nodes[index];
      labelIndexRef.current = index;
      el.textContent = node.count > 1 ? `${node.label} · ${node.count} 块` : node.label;
      el.style.display = 'block';
      placeLabel(index);
      if (canvas) canvas.style.cursor = 'pointer';
    };
    /** 悬停优先、否则回落到选中节点（选中后标签要一直挂着） */
    const refreshLabel = () => {
      const hover = hoverRef.current;
      if (hover >= 0) { showLabel(hover); return; }
      const sel = selectedIndexRef.current;
      showLabel(sel);
    };
    refreshLabelRef.current = refreshLabel;

    const rectNdc = (clientX: number, clientY: number) => {
      const rect = canvas.getBoundingClientRect();
      return new THREE.Vector2(
        ((clientX - rect.left) / rect.width) * 2 - 1,
        -(((clientY - rect.top) / rect.height) * 2 - 1),
      );
    };
    const raycaster = new THREE.Raycaster();

    const onPointerDown = (ev: PointerEvent) => {
      if (ev.button !== 0) {
        // 右键/中键 = 环绕平移：这也算"用户自己动了相机"
        ctx.userMoved = true;
        return;
      }
      const idx = pickAtRef.current(ev.clientX, ev.clientY);
      if (idx < 0) {
        // 空白处：交给 OrbitControls 做环绕/平移；记下按下点，抬手时若没动过就取消选中
        ctx.userMoved = true;
        emptyDownRef.current = { x: ev.clientX, y: ev.clientY };
        return;
      }
      emptyDownRef.current = null;
      const sn = sceneRef.current;
      if (!sn) return;
      controls.enabled = false;
      const nodePos = new THREE.Vector3(
        ctx.worldPos[idx * 3],
        ctx.worldPos[idx * 3 + 1],
        ctx.worldPos[idx * 3 + 2],
      );
      // 静止位置（布局算出来的那个）：邻居的位移量按它算，松手时被拖的那个也钉在**当下**的位置
      const a = ctx.settled?.get(sn.nodes[idx].id);
      const anchor = a ? new THREE.Vector3(a[0], a[1], a[2]) : nodePos.clone();
      // 关系节点 → 权重。按"静止间距"衰减：紧邻的跟着走一半，隔远的几乎不动。
      // 强度是拉动条读来的（0 = 完全不带动关系节点，那时连表都不用建）
      const attract = new Map<number, { w: number; anchor: THREE.Vector3 }>();
      const strength = paramsRef.current.attract;
      if (strength > 0) {
        for (const e of sn.edges) {
          const other = e.from === idx ? e.to : e.to === idx ? e.from : -1;
          if (other < 0 || attract.has(other)) continue;
          const oa = ctx.settled?.get(sn.nodes[other].id);
          if (!oa) continue;
          const own = new THREE.Vector3(oa[0], oa[1], oa[2]);
          const d = own.distanceTo(anchor);
          attract.set(other, { w: strength / (1 + (d / ATTRACT_RADIUS) ** 2), anchor: own });
        }
      }
      // 拖动平面与相机视线垂直 ⇒ 深度不变 ⇒ 节点大小不变，手感稳定
      const normal = camera.getWorldDirection(new THREE.Vector3()).negate();
      const plane = new THREE.Plane().setFromNormalAndCoplanarPoint(normal, nodePos);
      raycaster.setFromCamera(rectNdc(ev.clientX, ev.clientY), camera);
      const hit = new THREE.Vector3();
      const ok = raycaster.ray.intersectPlane(plane, hit);
      dragRef.current = {
        index: idx,
        moved: false,
        startX: ev.clientX,
        startY: ev.clientY,
        plane,
        anchor,
        attract,
        // 记下抓取偏移，否则一按下节点会瞬间跳到光标处
        grabOffset: ok ? nodePos.clone().sub(hit) : new THREE.Vector3(),
      };
      canvas.setPointerCapture(ev.pointerId);
      showLabel(idx);
      ev.stopPropagation();
    };

    const onPointerMove = (ev: PointerEvent) => {
      const drag = dragRef.current;
      if (drag) {
        // 位移超过阈值才算"拖动"，否则"点一下"会被判成 0 距离拖动、选中反而丢了
        if (Math.abs(ev.clientX - drag.startX) + Math.abs(ev.clientY - drag.startY) >= DRAG_SLOP) {
          drag.moved = true;
        }
        raycaster.setFromCamera(rectNdc(ev.clientX, ev.clientY), camera);
        const hit = new THREE.Vector3();
        if (raycaster.ray.intersectPlane(drag.plane, hit)) {
          hit.add(drag.grabOffset);
          const pos = ctx.worldPos;
          const dx = hit.x - drag.anchor.x;
          const dy = hit.y - drag.anchor.y;
          const dz = hit.z - drag.anchor.z;
          const moving: number[] = [drag.index];
          pos[drag.index * 3] = hit.x;
          pos[drag.index * 3 + 1] = hit.y;
          pos[drag.index * 3 + 2] = hit.z;
          labelIndexRef.current = drag.index;
          // 关系节点被"吸"着按权重跟过来：看起来是**整张网被拉动**，而不只是这一个点在跑
          for (const [idx, at] of drag.attract) {
            pos[idx * 3] = at.anchor.x + dx * at.w;
            pos[idx * 3 + 1] = at.anchor.y + dy * at.w;
            pos[idx * 3 + 2] = at.anchor.z + dz * at.w;
            moving.push(idx);
          }
          uploadNodesRef.current(moving);
        }
        return;
      }
      // 按住键（环绕/平移）时不做拾取：那会儿不需要悬停标签，拾取本身也不便宜
      if (ev.buttons !== 0) {
        hoverRef.current = -1;
        refreshLabel();
        return;
      }
      hoverRef.current = pickAtRef.current(ev.clientX, ev.clientY);
      refreshLabel();
    };

    const onPointerUp = (ev: PointerEvent) => {
      const drag = dragRef.current;
      const empty = emptyDownRef.current;
      dragRef.current = null;
      emptyDownRef.current = null;
      controls.enabled = true;
      try {
        if (canvas.hasPointerCapture(ev.pointerId)) canvas.releasePointerCapture(ev.pointerId);
      } catch { /* 指针已失效 */ }
      if (!drag) {
        // **点空白 = 取消选中**：按下与抬起之间没挪动（挪动就是把视角转了，不该丢选中）
        if (empty && Math.abs(ev.clientX - empty.x) + Math.abs(ev.clientY - empty.y) < DRAG_SLOP) {
          commitClickRef.current(-1);
        }
        return;
      }
      if (drag.moved) {
        // 分工是刻意的：**被拖的那个钉在你放下的位置**（那是你的显式意图），
        // **被"吸"过来的关系节点弹性弹回各自的静止位置**（它们不是你放的，只是被带了一下）。
        const node = sceneRef.current?.nodes[drag.index];
        const pos = ctx.worldPos;
        if (node) {
          const pinned: [number, number, number] = [
            pos[drag.index * 3],
            pos[drag.index * 3 + 1],
            pos[drag.index * 3 + 2],
          ];
          movedRef.current.set(node.id, pinned);
          // **必须同时更新 `settled`** —— 它是"这个节点现在实际在哪"的唯一口径。
          // 只写 `movedRef` 的话，`settled` 里留着的仍是**布局位置**，于是这个钉子一旦
          // 被邻居的吸附带动、松手时就会弹回**布局位置**而不是你放下的位置 ——
          // 表现就是"已移动的节点会被归位"（用户报的正是这个）。
          ctx.settled?.set(node.id, pinned);
          setMovedCount(movedRef.current.size);
        }
        for (const [idx, at] of drag.attract) {
          ctx.springs.set(idx, {
            pos: new THREE.Vector3(pos[idx * 3], pos[idx * 3 + 1], pos[idx * 3 + 2]),
            vel: new THREE.Vector3(),
            anchor: at.anchor.clone(),
          });
        }
        ctx.dirty = true;
      } else {
        commitClickRef.current(drag.index);
      }
      hoverRef.current = pickAtRef.current(ev.clientX, ev.clientY);
      refreshLabel();
    };

    const onPointerLeave = () => {
      hoverRef.current = -1;
      refreshLabel();
    };

    const onWheel = () => { ctx.userMoved = true; };

    canvas.addEventListener('pointerdown', onPointerDown, { capture: true });
    canvas.addEventListener('pointermove', onPointerMove);
    canvas.addEventListener('pointerup', onPointerUp);
    canvas.addEventListener('pointerleave', onPointerLeave);
    canvas.addEventListener('wheel', onWheel, { passive: true });

    /**
     * 推进所有动画：① 整图位置补间 ② 相机补间 ③ 单节点弹性回位。
     * 三者的共同点是**只改缓冲、不碰 React**，所以每帧成本就是"写一个 Float32Array + 一次上传" ——
     * 这正是换成 WebGL 之后才付得起的东西（旧版每帧改位置等于重画整块画布）。
     * 没有动画时这个函数什么都不做，空闲仍然零提交。
     */
    let prevNow = 0;
    /** 上一次加在相机上的"手抖"偏移：下一帧要先把它撤掉（见 tick） */
    const drift = new THREE.Vector3();
    const advance = (now: number) => {
      // dt 用真实帧间隔（夹在 1/240 ~ 1/30）：弹簧是靠积分推的，步长必须是真时间
      const dt = prevNow > 0 ? Math.min(1 / 30, Math.max(1 / 240, (now - prevNow) / 1000)) : 1 / 60;
      prevNow = now;
      const sn = sceneRef.current;

      // ① 位置补间
      const tw = ctx.tween;
      if (tw) {
        const pos = ctx.worldPos;
        const n = sn ? sn.nodes.length : 0;
        const t = Math.min(1, (now - tw.start) / tw.dur);
        const e = easeOutCubic(t);
        for (let i = 0; i < n * 3; i++) pos[i] = tw.from[i] + (tw.to[i] - tw.from[i]) * e;
        if (ctx.nodePos) {
          ctx.nodePos.addUpdateRange(0, n * 3);
          ctx.nodePos.needsUpdate = true;
        }
        const lp = ctx.linePos;
        if (lp && sn) {
          const arr = lp.array as Float32Array;
          const m = sn.edges.length;
          for (let i = 0; i < m; i++) {
            const ed = sn.edges[i];
            writeEdge(arr, i, ed.from, ed.to, pos);
          }
          lp.addUpdateRange(0, m * 6);
          lp.needsUpdate = true;
        }
        if (ctx.ring && selectedIndexRef.current >= 0 && selectedIndexRef.current < n) {
          const k = selectedIndexRef.current;
          ctx.ringPos.setXYZ(0, pos[k * 3], pos[k * 3 + 1], pos[k * 3 + 2]);
          ctx.ringPos.needsUpdate = true;
        }
        // 入场时让边"随后浮现"：节点先动起来，线在稍长一点的窗口里从 0 补到常态
        if (tw.fadeEdges) {
          ctx.lineMat.opacity = LINE_OPACITY * Math.min(1, (now - tw.start) / (tw.dur * 1.3));
        }
        if (t >= 1) {
          ctx.tween = null;
          ctx.lineMat.opacity = LINE_OPACITY;
        }
        ctx.dirty = true;
      }

      // ② 相机补间
      const ct = ctx.camTween;
      if (ct) {
        const t = Math.min(1, (now - ct.start) / ct.dur);
        const e = easeInOutCubic(t);
        camera.position.lerpVectors(ct.fromP, ct.toP, e);
        controls.target.lerpVectors(ct.fromT, ct.toT, e);
        controls.update();
        if (t >= 1) ctx.camTween = null;
        ctx.dirty = true;
      }

      // ③ 弹性回位：拖动松手后，被"吸"过来的**关系节点**各自弹回自己的静止位置
      if (ctx.springs.size > 0) {
        const k = SPRING_OMEGA * SPRING_OMEGA;
        const c = 2 * SPRING_ZETA * SPRING_OMEGA;
        const pos = ctx.worldPos;
        const moved: number[] = [];
        for (const [index, s] of ctx.springs) {
          // 半隐式欧拉：先更新速度再用新速度更新位置 —— 显式欧拉在弹簧上很容易发散
          s.vel.x += ((s.anchor.x - s.pos.x) * k - s.vel.x * c) * dt;
          s.vel.y += ((s.anchor.y - s.pos.y) * k - s.vel.y * c) * dt;
          s.vel.z += ((s.anchor.z - s.pos.z) * k - s.vel.z * c) * dt;
          s.pos.addScaledVector(s.vel, dt);
          if (s.pos.distanceToSquared(s.anchor) < 1e-4 && s.vel.lengthSq() < 1e-4) {
            s.pos.copy(s.anchor);
            ctx.springs.delete(index);
          }
          pos[index * 3] = s.pos.x;
          pos[index * 3 + 1] = s.pos.y;
          pos[index * 3 + 2] = s.pos.z;
          moved.push(index);
        }
        uploadNodesRef.current(moved);
        ctx.dirty = true;
      }
    };

    const tick = () => {
      ctx.raf = requestAnimationFrame(tick);
      const now = performance.now();
      // ① 撤掉上一帧的"手抖"，让补间与 OrbitControls 都在**干净**的相机位姿上工作
      camera.position.sub(drift);
      // ② 动画推进必须在 dirty 判断**之前**：它自己会置脏，否则补间跑到一半就停了
      advance(now);
      if (ctx.motion) {
        controls.update(); // ③ 相机环绕（`autoRotate` 由动效 effect 打开）
        ctx.mat.uniforms.uTime.value = now / 1000; // ④ 节点"眨眼"
        ctx.dirty = true;
      }
      // ⑤ 本帧"手抖"：只在动效开、且没有补间在抢相机的时候加（渲染与拾取都用这个位姿，才不会错位）
      if (ctx.motion && !ctx.camTween) {
        const amp = camera.position.distanceTo(controls.target) * paramsRef.current.drift;
        const t = now / 1000;
        drift.set(
          Math.sin(t * 0.29) * amp,
          Math.sin(t * 0.41 + 1.3) * amp,
          Math.sin(t * 0.23 + 2.1) * amp,
        );
      } else {
        drift.set(0, 0, 0);
      }
      camera.position.add(drift);
      if (!ctx.dirty) return;
      ctx.dirty = false;
      renderer.render(glScene, camera);
      // 相机一动，标签就得重新钉一次（否则自转时标签会从节点上滑走）
      if (labelIndexRef.current >= 0) placeLabel(labelIndexRef.current);
    };
    ctx.raf = requestAnimationFrame(tick);

    return () => {
      cancelAnimationFrame(ctx.raf);
      ro.disconnect();
      canvas.removeEventListener('pointerdown', onPointerDown, { capture: true });
      canvas.removeEventListener('pointermove', onPointerMove);
      canvas.removeEventListener('pointerup', onPointerUp);
      canvas.removeEventListener('pointerleave', onPointerLeave);
      canvas.removeEventListener('wheel', onWheel);
      controls.removeEventListener('change', markDirty);
      controls.dispose();
      ctx.points?.geometry.dispose();
      ctx.lines?.geometry.dispose();
      ringGeo.dispose();
      mat.dispose();
      lineMat.dispose();
      ringMat.dispose();
      renderer.dispose();
      glRef.current = null;
    };
  }, []);

  /* ── 图谱动效开关：一个布尔同时管相机环绕、相机悬浮、节点闪烁、（CSS）星空漂移 ── */
  useEffect(() => {
    const ctx = glRef.current;
    if (!ctx) return;
    ctx.motion = motionOn;
    // **`autoRotate` 必须显式打开**：`autoRotateSpeed` 只是个参数，
    // 光设它、不设 `autoRotate`，`controls.update()` 什么都不会转（曾经就这么错过一次）。
    ctx.controls.autoRotate = motionOn;
    ctx.mat.uniforms.uTwinkle.value = motionOn ? 1 : 0;
    ctx.dirty = true;
  }, [motionOn]);

  /* 参数变了就同步给 GL 现场：环绕速度与两个眨眼 uniform。
   * 手抖幅度与吸附强度不在依赖里 —— 它们分别在**每帧**（tick）与**每次按下**（pointerdown）现读
   * `paramsRef`，那样拖动滑块时不用重建任何东西。 */
  useEffect(() => {
    const ctx = glRef.current;
    if (!ctx) return;
    // OrbitControls 的口径：`autoRotateSpeed = 2.0` 约等于 30 秒一圈 ⇒ 除以 6 得到"度/秒"
    ctx.controls.autoRotateSpeed = params.rotateSpeed / 6;
    ctx.mat.uniforms.uTwinkleSpeed.value = params.twinkleSpeed;
    ctx.mat.uniforms.uTwinkleAmp.value = params.twinkleAmp;
    ctx.dirty = true;
  }, [params]);

  /* ── 数据 → GPU 缓冲 ── */
  useEffect(() => {
    const ctx = glRef.current;
    if (!ctx) return;
    const n = scene.nodes.length;

    // 目标位置（布局算出来的）；世界坐标的当前值另存一份，补间就在这两份之间插值
    const to = new Float32Array(scene.positions);
    // 被手工拖过并钉住的节点：目标位置就是它被放下的地方（换布局也不动它）
    for (let i = 0; i < n; i++) {
      const o = movedRef.current.get(scene.nodes[i].id);
      if (!o) continue;
      to[i * 3] = o[0];
      to[i * 3 + 1] = o[1];
      to[i * 3 + 2] = o[2];
    }
    const pos = new Float32Array(to.length);
    const from = new Float32Array(to.length);
    const isEntrance = ctx.settled === null || ctx.settledBase !== base.id;
    if (isEntrance) {
      // 入场：从小球长开（不是从原点炸开 —— 那样头几帧是一坨刺眼亮斑）
      for (let i = 0; i < to.length; i++) from[i] = to[i] * 0.15;
    } else {
      // 换布局：**按节点 id** 找"从哪出发"。老节点从原位出发（看起来是滑过去），
      // 新节点（刚展开的分块）从**宿主文件**的位置长出来（看起来是从文件里冒出来）。
      const settled = ctx.settled as Map<string, [number, number, number]>;
      const hostOf = new Map<number, number>();
      for (const e of scene.edges) if (e.kind === 'contains') hostOf.set(e.to, e.from);
      for (let i = 0; i < n; i++) {
        const prev = settled.get(scene.nodes[i].id);
        const h = hostOf.get(i);
        if (prev) {
          from[i * 3] = prev[0];
          from[i * 3 + 1] = prev[1];
          from[i * 3 + 2] = prev[2];
        } else if (h !== undefined) {
          from[i * 3] = to[h * 3];
          from[i * 3 + 1] = to[h * 3 + 1];
          from[i * 3 + 2] = to[h * 3 + 2];
        }
        // 其余保持 0（孤立的新节点从原点出现，罕见）
      }
    }
    pos.set(from);
    ctx.worldPos = pos;
    // 记下这一轮的稳定位置（下一次换布局就从这里出发）
    ctx.settled = new Map(
      scene.nodes.map((node, i) => [node.id, [to[i * 3], to[i * 3 + 1], to[i * 3 + 2]] as [number, number, number]]),
    );
    ctx.settledBase = base.id;
    ctx.tween = { from, to, start: performance.now(), dur: TWEEN_MS, fadeEdges: isEntrance };
    if (isEntrance) ctx.lineMat.opacity = 0;
    ctx.springs.clear(); // 换布局时旧的弹性回位已经没意义了

    const color = new Float32Array(n * 3);
    const size = new Float32Array(n);
    // 每颗星自己的相位（黄金角散布）：闪烁才不会整张图一起亮一起暗，而是"一片在眨"
    const phase = new Float32Array(n);
    const tmpColor = new THREE.Color();
    for (let i = 0; i < n; i++) {
      tmpColor.setHex(originHex(scene.nodes[i]));
      color[i * 3] = tmpColor.r;
      color[i * 3 + 1] = tmpColor.g;
      color[i * 3 + 2] = tmpColor.b;
      size[i] = scene.nodes[i].radius;
      phase[i] = (i * PHASE_STRIDE) % (Math.PI * 2);
    }
    const pg = new THREE.BufferGeometry();
    ctx.nodePos = new THREE.BufferAttribute(pos, 3);
    pg.setAttribute('position', ctx.nodePos);
    pg.setAttribute('aColor', new THREE.BufferAttribute(color, 3));
    pg.setAttribute('aSize', new THREE.BufferAttribute(size, 1));
    pg.setAttribute('aPhase', new THREE.BufferAttribute(phase, 1));

    ctx.points?.geometry.dispose();
    ctx.group.remove(ctx.points as THREE.Object3D);
    const points = new THREE.Points(pg, ctx.mat);
    // 整张图都在原点附近、又常常铺满视口，视锥剔除只会白算包围球
    points.frustumCulled = false;
    ctx.points = points;
    ctx.group.add(points);

    // 边：每边两个顶点，逐顶点颜色
    const e = scene.edges.length;
    const lp = new Float32Array(e * 6);
    const lc = new Float32Array(e * 6);
    for (let i = 0; i < e; i++) {
      const { from, to, kind } = scene.edges[i];
      writeEdge(lp, i, from, to, pos);
      tmpColor.setHex(kindHex(kind));
      for (let v = 0; v < 2; v++) {
        lc[i * 6 + v * 3] = tmpColor.r;
        lc[i * 6 + v * 3 + 1] = tmpColor.g;
        lc[i * 6 + v * 3 + 2] = tmpColor.b;
      }
    }
    const lg = new THREE.BufferGeometry();
    ctx.linePos = new THREE.BufferAttribute(lp, 3);
    lg.setAttribute('position', ctx.linePos);
    lg.setAttribute('color', new THREE.BufferAttribute(lc, 3));
    ctx.edgeCount = e;

    ctx.lines?.geometry.dispose();
    ctx.group.remove(ctx.lines as THREE.Object3D);
    const lines = new THREE.LineSegments(lg, ctx.lineMat);
    lines.frustumCulled = false;
    ctx.lines = lines;
    ctx.group.add(lines);

    ctx.dirty = true;
    // 换数据后选中/聚焦的节点可能已经不在场景里
    selectedIndexRef.current = selectedId ? scene.nodes.findIndex((x) => x.id === selectedId) : -1;
    syncRingRef.current();
    refreshLabelRef.current();
    // syncRingRef / refreshLabelRef 都是命令式读取，不参与依赖；
    // `base.id` 用来判断这一次是不是入场，`sceneRebuild` 给"归位"用
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [scene, base.id, sceneRebuild]);

  // 换库：清掉选中（取景与"用户动过相机"的标记在下面的取景 effect 里重置）
  const [prevBaseId, setPrevBaseId] = useState(base.id);
  if (prevBaseId !== base.id) {
    setPrevBaseId(base.id);
    setSelectedId(null);
  }

  /* ── 命中测试：把节点投影到屏幕取离指针最近的一个 ──
   * 不用 raycaster：`Points` 的射线检测本质也是逐点算，自己算还省一次遍历包装。
   * 三条硬约束都是实测逼出来的（2 万节点时逐个 `Vector3.project` 要 ~3.6ms）：
   *   ① 视图投影矩阵**只乘一次**、每点只做 4 次点积；② 全程零分配；③ 按住鼠标时直接跳过。 */
  const pickAt = (clientX: number, clientY: number): number => {
    const ctx = glRef.current;
    const canvas = canvasRef.current;
    const sn = sceneRef.current;
    if (!ctx || !canvas || !sn || ctx.worldPos.length === 0) return -1;
    const rect = canvas.getBoundingClientRect();
    const mx = clientX - rect.left;
    const my = clientY - rect.top;
    const cam = ctx.camera;
    cam.updateMatrixWorld();
    _mwi.copy(cam.matrixWorld).invert();
    const m = _vp.multiplyMatrices(cam.projectionMatrix, _mwi).elements;
    const pos = ctx.worldPos;
    const halfW = rect.width * 0.5;
    const halfH = rect.height * 0.5;
    let best = -1;
    let bestD = PICK_RADIUS * PICK_RADIUS;
    for (let i = 0; i < sn.nodes.length; i++) {
      const px = pos[i * 3];
      const py = pos[i * 3 + 1];
      const pz = pos[i * 3 + 2];
      const cw = m[3] * px + m[7] * py + m[11] * pz + m[15];
      if (cw <= 0) continue; // 在相机背后
      const dx = (m[0] * px + m[4] * py + m[8] * pz + m[12]) / cw * halfW + halfW - mx;
      const dy = -((m[1] * px + m[5] * py + m[9] * pz + m[13]) / cw) * halfH + halfH - my;
      const d = dx * dx + dy * dy;
      if (d < bestD) { bestD = d; best = i; }
    }
    return best;
  };

  /* ── 选中环：全部走命令式，不进 React（拖动时每帧都要更新） ── */
  const syncRing = () => {
    const ctx = glRef.current;
    const sn = sceneRef.current;
    if (!ctx || !sn) return;
    const i = selectedIndexRef.current;
    if (i < 0 || i >= sn.nodes.length) {
      ctx.ring.visible = false;
      return;
    }
    ctx.ring.visible = true;
    ctx.ringPos.setXYZ(0, ctx.worldPos[i * 3], ctx.worldPos[i * 3 + 1], ctx.worldPos[i * 3 + 2]);
    ctx.ringPos.needsUpdate = true;
    // 描边的**宽度固定**（`RING_STROKE_PX`），这里只告诉着色器球本身有多大
    (ctx.ringMat.uniforms.uNodeWorld as { value: number }).value = sn.nodes[i].radius;
    const accent = readAccentHex();
    (ctx.ringMat.uniforms.uColor as { value: THREE.Color }).value.setHex(accent);
    ctx.dirty = true;
  };

  /**
   * 把若干个节点的新位置上传（位置**已经写进** `ctx.worldPos`）。
   *
   * 收成"一批"是因为：拖动时被"吸"过来的关系节点可能有十几个，逐个调 `moveNode` 会把边表
   * 扫描十几遍（大库上是几十万次/帧）。这里一次调用只扫**一遍**边。
   */
  const uploadNodes = (indices: number[]) => {
    const ctx = glRef.current;
    const sn = sceneRef.current;
    if (!ctx || !sn || indices.length === 0) return;
    const pos = ctx.worldPos;
    if (ctx.nodePos) {
      for (const i of indices) ctx.nodePos.addUpdateRange(i * 3, 3);
      ctx.nodePos.needsUpdate = true;
    }
    const moving = new Set(indices);
    const lp = ctx.linePos;
    if (lp) {
      const arr = lp.array as Float32Array;
      let touched = 0;
      for (let i = 0; i < sn.edges.length; i++) {
        const e = sn.edges[i];
        if (!moving.has(e.from) && !moving.has(e.to)) continue;
        writeEdge(arr, i, e.from, e.to, pos);
        lp.addUpdateRange(i * 6, 6);
        touched += 1;
      }
      if (touched > 0) lp.needsUpdate = true;
    }
    const sel = selectedIndexRef.current;
    if (sel >= 0 && moving.has(sel)) {
      ctx.ringPos.setXYZ(0, pos[sel * 3], pos[sel * 3 + 1], pos[sel * 3 + 2]);
      ctx.ringPos.needsUpdate = true;
    }
    ctx.dirty = true;
    refreshLabelRef.current();
  };

  /** 移动一个节点（拖动路径用）：写位置 + 上传；标签跟着被拖的那个走 */
  const moveNode = (index: number, x: number, y: number, z: number) => {
    const ctx = glRef.current;
    const sn = sceneRef.current;
    if (!ctx || !sn || index < 0 || index >= sn.nodes.length) return;
    const pos = ctx.worldPos;
    pos[index * 3] = x;
    pos[index * 3 + 1] = y;
    pos[index * 3 + 2] = z;
    labelIndexRef.current = index;
    uploadNodes([index]);
  };

  /**
   * 相机带缓动飞过去（点节点 / 重置视角 / 自动取景都走它）。
   * 不直接改 `camera.position` 是**故意的**：瞬移的视角会让人丢失空间感 ——
   * 缓动既让人看清"我从哪移到哪"，也让连续几次取景自然衔接。
   */
  const flyTo = (target: THREE.Vector3, distance: number, dir?: THREE.Vector3, dur = CAM_TWEEN_MS) => {
    const ctx = glRef.current;
    if (!ctx) return;
    const keep = dir
      ? dir.clone().normalize()
      : ctx.camera.position.clone().sub(ctx.controls.target);
    if (keep.lengthSq() < 1e-6) keep.set(0, 0, 1);
    ctx.camTween = {
      fromP: ctx.camera.position.clone(),
      toP: target.clone().addScaledVector(keep.normalize(), distance),
      fromT: ctx.controls.target.clone(),
      toT: target.clone(),
      start: performance.now(),
      dur,
    };
    ctx.dirty = true;
  };

  /** 点击（未拖动的按下-抬起）：选中；文件节点顺带展开它自己的分块。index < 0 = 点空白 ⇒ 取消选中 */
  const commitClick = (index: number) => {
    const ctx = glRef.current;
    const sn = sceneRef.current;
    if (!ctx || !sn || index < 0 || index >= sn.nodes.length) {
      selectedIndexRef.current = -1;
      setSelectedId(null);
      syncRing();
      refreshLabelRef.current();
      return;
    }
    const node = sn.nodes[index];
    selectedIndexRef.current = index;
    setSelectedId(node.id);
    syncRing();
    // 选中即聚焦：相机缓动过去（保留当前距离与朝向）。
    // 位置取 `settled` 而不是 `worldPos` —— 补间进行中 worldPos 还在路上，飞向一个移动目标会飘。
    const anchor = ctx.settled?.get(node.id);
    flyTo(
      new THREE.Vector3(
        anchor ? anchor[0] : ctx.worldPos[index * 3],
        anchor ? anchor[1] : ctx.worldPos[index * 3 + 1],
        anchor ? anchor[2] : ctx.worldPos[index * 3 + 2],
      ),
      ctx.camera.position.distanceTo(ctx.controls.target),
    );
    // 大库默认不展开分块：点一个文件就展开**它自己**（幂等），于是它旁边立刻长出分块。
    // 读 store 现取而不是用 `expanded` 闭包 —— 这个函数是命令式的，闭包可能过期。
    if (node.refId?.startsWith('file:')) {
      const src = nodeById.get(node.refId);
      const path = src?.sourceRef;
      if (path && src && src.chunkCount > 0 && !useKnowledgeStore.getState().graphChunks[path]) {
        void expandGraphFiles([{ relPath: path, chunkCount: src.chunkCount }], { silent: true });
      }
    }
    refreshLabelRef.current();
  };

  /* 把上面三个命令式操作装进 ref（原生指针事件通过 ref 调它们，见 pickAtRef 的说明）。
     不写依赖数组：每次渲染都刷新一份，闭包里的 nodeById / 状态永远是最新的。 */
  useEffect(() => {
    pickAtRef.current = pickAt;
    moveNodeRef.current = moveNode;
    uploadNodesRef.current = uploadNodes;
    commitClickRef.current = commitClick;
    syncRingRef.current = syncRing;
  });

  /* ── 按内容取景：默认缩放由**外接球半径**决定，而不是写死的常数 ──
   * 十几个节点的库不该被摆在画布中心当一小撮；自动展开分块会让半径逐步变大，
   * 所以这里跟着半径走（表现为"图慢慢铺开"），**但用户一旦自己动过相机就再也不插手**。
   * 状态放在 `ctx` 而不是组件 ref：GL 现场重建（StrictMode 二次挂载）时必须重新取景，
   * 详见 GlCtx 里 `userMoved` 的说明。 */
  useEffect(() => {
    const ctx = glRef.current;
    if (!ctx || scene.nodes.length === 0) return;
    if (ctx.fitBase !== base.id) {
      ctx.fitBase = base.id;
      ctx.fitRadius = 0;
      ctx.userMoved = false;
    } else if (ctx.userMoved) {
      return;
    }
    // 改动不到 10% 就不重取景，免得自动展开过程中相机一直在微调
    if (ctx.fitRadius > 0 && Math.abs(scene.radius - ctx.fitRadius) / ctx.fitRadius < 0.1) return;
    ctx.fitRadius = scene.radius;
    flyTo(new THREE.Vector3(0, 0, 0), fitDistance(scene.radius, ctx.camera.fov), new THREE.Vector3(0, 0, 1), 600);
  }, [scene.radius, scene.nodes.length, base.id]);

  /* 选中态变化（含从详情面板里点的"关系"跳转）时同步环与标签 */
  useEffect(() => {
    const sn = sceneRef.current;
    selectedIndexRef.current = sn && selectedId ? sn.nodes.findIndex((x) => x.id === selectedId) : -1;
    syncRingRef.current();
    refreshLabelRef.current();
  }, [selectedId]);

  const resetView = () => {
    const ctx = glRef.current;
    if (!ctx) return;
    // 重置 = 回到"按内容自动取景"，所以把"用户动过相机"的标记也清掉
    ctx.userMoved = false;
    ctx.fitRadius = 0;
    flyTo(new THREE.Vector3(0, 0, 0), fitDistance(scene.radius, ctx.camera.fov), new THREE.Vector3(0, 0, 1));
    setSelectedId(null);
  };

  /** 清掉被钉住的节点：布局是"算出来的"，被拖歪了要能一键回到原样 */
  const clearMoved = () => {
    movedRef.current.clear();
    setMovedCount(0);
    setSceneRebuild((v) => v + 1);
  };

  const toggleKind = (k: LinkKind) =>
    setEnabledKinds((prev) => (prev.includes(k) ? prev.filter((x) => x !== k) : [...prev, k]));

  const expandedCount = expanded.size;

  /* ── 详情：选中节点在核心图里的原始记录（文件/独立条目）或分块的内容 ── */
  const selectedCore: KnowledgeGraphNode | null = selectedId ? nodeById.get(selectedId) ?? null : null;
  const selectedChunk: KnowledgeEntry | null = useMemo(() => {
    if (selectedCore || !selectedId?.startsWith('chunk:')) return null;
    const key = selectedId.slice('chunk:'.length);
    for (const list of expanded.values()) {
      const hit = list.find((e) => e.key === key);
      if (hit) return hit;
    }
    return null;
  }, [selectedId, selectedCore, expanded]);
  const selectedScene: SceneNode | null = scene.nodes.find((n) => n.id === selectedId) ?? null;

  /** 选中节点的关联边（关系列表）：直接从场景边表里取，不用另建索引 */
  const selectedLinks = useMemo(() => {
    if (!selectedId) return [] as { id: string; other: string; label: string; kinds: LinkKind[] }[];
    const idx = scene.nodes.findIndex((n) => n.id === selectedId);
    if (idx < 0) return [];
    const out: { id: string; other: string; label: string; kinds: LinkKind[] }[] = [];
    for (const e of scene.edges) {
      const other = e.from === idx ? e.to : e.to === idx ? e.from : -1;
      if (other < 0) continue;
      out.push({
        id: `${e.from}__${e.to}__${e.kind}`,
        other: scene.nodes[other].id,
        label: scene.nodes[other].label,
        kinds: [e.kind],
      });
    }
    return out;
  }, [scene, selectedId]);

  const linkCount = useMemo(() => {
    const m = new Map<LinkKind, number>();
    for (const e of scene.edges) m.set(e.kind, (m.get(e.kind) ?? 0) + 1);
    return m;
  }, [scene.edges]);

  const [toastTip, setToastTip] = useState(() => localStorage.getItem(TOOLTIP_KEY) !== '0');

  return (
    <div className="flex-1 flex overflow-hidden">
      {/* 左：档位 / 边开关 / 视图 */}
      {/*
        256px 是"与上方 tab 行末端对齐"的结果，不是随手取的数：
        页头 `px-4`（16）+ 4 个 tab × 57px + 3 × gap 4px = 256。
        tab 的 57px = px-2.5 两侧 20 + 图标 11 + gap 4 + 两个汉字的 22。
        改 tab 文案 / 内边距时这个数要跟着改；「投喂」上挂了待确认徽标时 tab 行会更长，
        此时不再严格对齐（徽标宽度随数字位数变化，对齐不了）。
      */}
      <div className="w-[256px] shrink-0 overflow-y-auto p-3 space-y-3" style={{ backgroundColor: 'var(--bg-side)' }}>
        <div>
          <BlockTitle icon={Layers} text="分块展开" right={`${expandedCount} / ${expandTargets.length} 个文件`} />
          <div className="grid grid-cols-2 gap-1.5">
            <button
              onClick={() => void expandGraphFiles(expandTargets, { silent: true })}
              disabled={expandTargets.length === 0 || expandedCount >= expandTargets.length}
              className="justify-center px-1 py-1 rounded-lg text-[10px]"
              style={{
                color: 'var(--text-secondary)', backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)',
                opacity: expandTargets.length === 0 || expandedCount >= expandTargets.length ? 0.5 : 1,
              }}
            >
              展开全部分块
            </button>
            <button
              onClick={() => collapseGraphFiles()}
              disabled={expandedCount === 0}
              className="justify-center px-1 py-1 rounded-lg text-[10px]"
              style={{
                color: 'var(--text-secondary)', backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)',
                opacity: expandedCount === 0 ? 0.5 : 1,
              }}
            >
              收起全部分块
            </button>
          </div>
          <div className="mt-1.5 text-[10px] leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
            小库（文件 + 分块 ≤ {AUTO_EXPAND_MAX_NODES} 个节点）默认全展开；大库默认只画文件，
            点某个文件才展开它自己。要缩小规模请用「知识」列表的筛选。
          </div>
        </div>

        {/* 节点图例：颜色是**唯一**区分来源的手段（大小表达的是"有多少分块"），
            没有它就只能靠猜 —— 边有图例，节点更该有 */}
        <div>
          <BlockTitle icon={Palette} text="节点（颜色 = 来源）" />
          {/* 两列排：6 条来源单列会拖出很长一竖、右侧大片留白；两列后整块只剩三行。
              标签加 truncate 兜底 —— 来源名以后变长时截断，好过换行把两列的基线错开 */}
          <div className="grid grid-cols-2 gap-x-2 gap-y-1">
            {(Object.keys(ORIGIN_COLOR) as KnowledgeOrigin[]).map((o) => (
              <div key={o} className="flex items-center gap-1.5 min-w-0 text-[11px]" style={{ color: 'var(--text-secondary)' }}>
                <span className="w-2.5 h-2.5 rounded-full shrink-0" style={{ backgroundColor: ORIGIN_COLOR[o] }} />
                <span className="truncate">{ORIGIN_LABEL[o]}</span>
              </div>
            ))}
          </div>
          <div className="mt-1.5 text-[10px] leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
            圆越大 = 该文件登记的分块越多；小圆点是分块本身。选中的节点由主题色描边标出。
          </div>
        </div>

        <div>
          <BlockTitle icon={Share2} text="连线（可开关）" />
          <div className="space-y-1">
            {LEGEND.map((k) => {
              const on = enabledKinds.includes(k);
              return (
                <button
                  key={k}
                  onClick={() => toggleKind(k)}
                  className="w-full flex items-center gap-1.5 px-1.5 py-1 rounded text-[11px] transition-colors"
                  style={{
                    color: on ? 'var(--text-primary)' : 'var(--text-tertiary)',
                    backgroundColor: on ? 'var(--bg-tertiary)' : 'transparent',
                  }}
                >
                  <span className="w-3 h-[2px] rounded shrink-0" style={{ backgroundColor: on ? kindColor(k) : 'var(--border)' }} />
                  {kindLabel(k)}
                  <span className="ml-auto text-[10px]" style={{ color: 'var(--text-tertiary)' }}>{linkCount.get(k) ?? 0}</span>
                </button>
              );
            })}
          </div>
          <div className="mt-1.5 text-[10px] leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
            「分块归属」= 文件 → 它自己的分块；「共标签」里也包含「分块 → 别的文件」的跨文件连线
            （每个分块最多 3 条，只在展开了分块时出现）。
          </div>
        </div>

        {/* 分隔：上面是**业务参数**（图里显示什么：展开哪些分块、画哪些边、节点是什么来源），
            下面是**视觉参数**（看起来什么样：视角、动效、动画强度）。
            两类混在一起时，想找"能改什么"得一行行读过去。 */}
        <div className="border-t" style={{ borderColor: 'var(--border)' }} />

        <div>
          <BlockTitle icon={Eye} text="视图" />
          <div className="grid grid-cols-2 gap-1.5">
            <button
              onClick={resetView}
              className="flex items-center justify-center gap-1 px-1 py-1 rounded-lg text-[10px]"
              style={{ color: 'var(--text-secondary)', backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)' }}
              title="回到初始视角并取消选中"
            >
              <RotateCcw size={10} />
              重置视角
            </button>
            <button
              onClick={clearMoved}
              disabled={movedCount === 0}
              className="flex items-center justify-center gap-1 px-1 py-1 rounded-lg text-[10px]"
              style={{
                color: 'var(--text-secondary)', backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)',
                opacity: movedCount === 0 ? 0.5 : 1,
                cursor: movedCount === 0 ? 'not-allowed' : 'pointer',
              }}
              title={`把拖走的节点放回布局位置（当前 ${movedCount} 个）`}
            >
              <Trash2 size={10} />
              归位{movedCount > 0 ? ` (${movedCount})` : ''}
            </button>
          </div>
          <div className="mt-1.5 text-[10px] leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
            拖动节点 = 拖它（关系节点会被吸着跟过来，松手各自弹回）· 拖动空白 = 环绕 · 滚轮 = 缩放
          </div>
        </div>

        <div>
          <BlockTitle icon={Sparkles} text="图谱动效" right={`系统${systemReduce ? '已关闭' : '已开启'}`} />
          <div className="grid grid-cols-3 gap-1">
            {MOTION_OPTIONS.map((o) => {
              const on = motionPref === o.value;
              return (
                <button
                  key={o.value}
                  onClick={() => chooseMotion(o.value)}
                  /* 全局 `button` 是 inline-flex，不写 justify-center 文字会贴左 */
                  className="justify-center px-1 py-1 rounded-lg text-[10px] truncate"
                  style={{
                    color: on ? 'var(--accent)' : 'var(--text-secondary)',
                    backgroundColor: 'var(--bg-tertiary)',
                    border: `1px solid ${on ? 'var(--accent)' : 'var(--border)'}`,
                  }}
                  title={
                    o.value === 'auto' ? '跟随系统「动画效果」设置'
                      : o.value === 'on' ? `相机以约 ${params.rotateSpeed}°/s 环绕 + 节点各自缓慢明暗起伏`
                        : '全部静止（空闲时不提交任何帧）'
                  }
                >
                  {o.label}
                </button>
              );
            })}
          </div>
          <div className="mt-1.5 text-[10px] leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
            相机在环绕、节点在各自「眨眼」、背景星点在漂移 —— 三者同一个开关；关掉后一帧都不提交。
          </div>
        </div>

        {/* 参数：放在面板**最下面** —— 它们是"调味"而不是主流程，拧坏了有「恢复默认参数」兜着。
            持久化走 localStorage（与动效开关、文件分组方式同一套），不进数据库。 */}
        <div>
          <BlockTitle icon={SlidersHorizontal} text="动效参数" right="拖动即生效 · 自动记住" />
          <div className="space-y-1.5">
            {PARAM_SPECS.map((s) => (
              <div key={s.key}>
                <div className="flex items-center justify-between text-[10px] mb-0.5">
                  <span style={{ color: 'var(--text-secondary)' }}>{s.label}</span>
                  <span className="tabular-nums" style={{ color: 'var(--text-tertiary)' }}>{s.fmt(params[s.key])}</span>
                </div>
                <input
                  type="range"
                  min={s.min}
                  max={s.max}
                  step={s.step}
                  value={params[s.key]}
                  onChange={(e) => updateParam(s.key, Number(e.target.value))}
                  className="w-full"
                  style={{ accentColor: 'var(--accent)' }}
                />
              </div>
            ))}
          </div>
          <button
            onClick={resetParams}
            disabled={!paramsDirty}
            className="mt-2 w-full flex items-center justify-center gap-1 px-1 py-1 rounded-lg text-[10px]"
            style={{
              color: 'var(--text-secondary)', backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)',
              opacity: paramsDirty ? 1 : 0.5,
              cursor: paramsDirty ? 'pointer' : 'not-allowed',
            }}
            title="把这五个参数恢复到内置默认值"
          >
            <RotateCcw size={10} />
            恢复默认参数
          </button>
          <div className="mt-1.5 text-[10px] leading-relaxed" style={{ color: 'var(--text-tertiary)' }}>
            「眨眼频率」是角频率（默认 1.7/s ≈ 3.7 秒一轮）。「吸附强度」调到 0，拖动不带动关系节点。
          </div>
        </div>
      </div>

      {/* 中：画布 */}
      <div
        ref={hostRef}
        className={motionOn ? 'flex-1 relative overflow-hidden pd-motion' : 'flex-1 relative overflow-hidden'}
      >
        <div className="pd-graph-sky" aria-hidden />
        <div className="pd-graph-sky-bright" aria-hidden />

        <div className="absolute top-2 left-3 text-[10px] z-10 pointer-events-none" style={{ color: 'var(--text-tertiary)' }}>
          {`已显示 ${scene.shown} 个节点、${scene.edges.length} 条边`}
          {` · 已展开 ${expandedCount} 个文件的分块`}
          {scene.total > scene.shown && ` · 共 ${scene.total} 个，其余被安全阀截断`}
          {filtersActive && <span> · 已按筛选条件收窄</span>}
          {selectedId && <span>{` · 已选中`}</span>}
        </div>

        {/* 按需标签：绝对定位的 HTML，全程只有这一个元素 */}
        <div
          ref={labelRef}
          className="absolute top-0 left-0 z-20 pointer-events-none px-1.5 py-0.5 rounded text-[11px] whitespace-nowrap"
          style={{
            display: 'none',
            backgroundColor: 'var(--bg-secondary)',
            border: '1px solid var(--border)',
            color: 'var(--text-primary)',
          }}
        />

        <canvas ref={canvasRef} className="block w-full h-full relative" style={{ touchAction: 'none' }} />

        {scene.nodes.length === 0 && (
          <div className="absolute inset-0 flex flex-col items-center justify-center gap-2 pointer-events-none">
            <Crosshair size={20} style={{ color: 'var(--text-tertiary)' }} />
            <div className="text-xs" style={{ color: 'var(--text-tertiary)' }}>这个库里还没有可显示的节点</div>
          </div>
        )}

        {toastTip && (
          <button
            onClick={() => { setToastTip(false); localStorage.setItem(TOOLTIP_KEY, '0'); }}
            className="absolute bottom-2 left-3 z-10 px-2 py-1 rounded text-[10px]"
            style={{ color: 'var(--text-tertiary)', backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
            title="点一下不再显示"
          >
            拖动节点可挪位置 · 点空白取消选中 · 悬停看名称
          </button>
        )}

        {(selectedCore || selectedChunk || selectedScene) && (
          <DetailPanel
            base={base}
            node={selectedCore}
            chunk={selectedChunk}
            sceneNode={selectedScene}
            links={selectedLinks}
            onJump={(id) => setSelectedId(id)}
            onClose={() => setSelectedId(null)}
            onGoEntries={() => setTab('entries')}
          />
        )}
      </div>
    </div>
  );
}

/* ────────────────────────── 工具 ────────────────────────── */

const MAX_LINK_ROWS = 8;

/** 把第 i 条边的两个端点写进线段位置数组 */
function writeEdge(arr: Float32Array, i: number, from: number, to: number, pos: Float32Array): void {
  arr[i * 6] = pos[from * 3];
  arr[i * 6 + 1] = pos[from * 3 + 1];
  arr[i * 6 + 2] = pos[from * 3 + 2];
  arr[i * 6 + 3] = pos[to * 3];
  arr[i * 6 + 4] = pos[to * 3 + 1];
  arr[i * 6 + 5] = pos[to * 3 + 2];
}

/** 来源色 → WebGL 用的整数色。**从 `ORIGIN_COLOR` 现算**，不另抄一份表：抄一份必然漂移 */
const ORIGIN_HEX: Record<string, number> = Object.fromEntries(
  Object.entries(ORIGIN_COLOR).map(([k, v]) => [k, parseInt(v.slice(1), 16)]),
);
const originHex = (n: SceneNode) => ORIGIN_HEX[n.origin] ?? 0x888888;

/** 主题强调色（选中环用它）：从 CSS 变量现读，换主题后下一次同步就生效 */
function readAccentHex(): number {
  const v = getComputedStyle(document.documentElement).getPropertyValue('--accent').trim();
  const m = /^#([0-9a-f]{6})$/i.exec(v);
  return m ? parseInt(m[1], 16) : 0x38bdf8;
}

interface DetailProps {
  base: KnowledgeBase;
  node: KnowledgeGraphNode | null;
  chunk: KnowledgeEntry | null;
  sceneNode: SceneNode | null;
  links: { id: string; other: string; label: string; kinds: LinkKind[] }[];
  onJump: (id: string) => void;
  onClose: () => void;
  onGoEntries: () => void;
}

/** 详情面板：内容全部来自核心图/分块的**原始记录**，不是渲染层的那点信息 */
function DetailPanel({ base, node, chunk, sceneNode, links, onJump, onClose, onGoEntries }: DetailProps) {
  const label = node?.label ?? (chunk ? entryTitle(chunk.key) : sceneNode?.label ?? '');
  const tags = node?.tags ?? chunk?.tags ?? [];
  return (
    <div
      className="absolute top-2 right-2 bottom-2 w-[264px] rounded-lg overflow-y-auto p-3 space-y-3 shadow-lg"
      style={{ backgroundColor: 'var(--bg-secondary)', border: '1px solid var(--border)' }}
    >
      <div className="flex items-start gap-2">
        <div className="flex-1 min-w-0">
          <div className="text-xs font-medium break-all" style={{ color: 'var(--text-primary)' }}>{label}</div>
          <div className="flex items-center gap-1.5 mt-1 text-[10px] flex-wrap" style={{ color: 'var(--text-tertiary)' }}>
            {sceneNode && <span>{ORIGIN_LABEL[sceneNode.origin]}</span>}
            {node?.kind === 'file' && <span>· {node.chunkCount} 个分块</span>}
            {chunk && <span>· 文件分块</span>}
          </div>
        </div>
        <button
          onClick={onClose}
          className="pd-btn shrink-0 text-[10px] px-1.5 py-0.5 rounded"
          style={{ color: 'var(--text-tertiary)', backgroundColor: 'var(--bg-tertiary)' }}
        >
          取消
        </button>
      </div>

      {node?.sourceRef && (
        <div className="text-[10px] break-all" style={{ color: 'var(--text-tertiary)' }}>来源：{node.sourceRef}</div>
      )}

      {node && (
        <div className="space-y-1">
          {base.fields.map((f) => {
            const v = node.meta[f.key];
            if (v === undefined) return null;
            return (
              <div key={f.key} className="flex items-baseline gap-2 text-[10px]">
                <span className="shrink-0" style={{ color: 'var(--text-tertiary)' }}>{f.label}</span>
                <span className="min-w-0 break-all" style={{ color: v === null ? '#F59E0B' : 'var(--text-secondary)' }}>
                  {v === null ? '多值（各分块不一致）' : v}
                </span>
              </div>
            );
          })}
        </div>
      )}

      {tags.length > 0 && (
        <div className="flex items-center gap-1 flex-wrap">
          {tags.slice(0, 12).map((t) => (
            <span key={t} className="text-[9px] px-1 rounded" style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}>
              #{t}
            </span>
          ))}
          {tags.length > 12 && <span className="text-[9px]" style={{ color: 'var(--text-tertiary)' }}>…共 {tags.length} 个</span>}
        </div>
      )}

      {chunk && (
        <div className="text-[11px] leading-relaxed whitespace-pre-wrap rounded-lg p-2.5" style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-primary)' }}>
          {chunk.value}
        </div>
      )}

      {node && node.entries.length > 0 && (
        <div className="space-y-1.5">
          <div className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>前 3 块</div>
          {node.entries.slice(0, 3).map((e) => (
            <div
              key={e.key}
              className="text-[10px] leading-relaxed rounded-lg p-2 line-clamp-3"
              style={{ backgroundColor: 'var(--bg-tertiary)', color: 'var(--text-secondary)' }}
              title={entryTitle(e.key)}
            >
              {e.value.slice(0, 160)}
            </div>
          ))}
        </div>
      )}

      {/* 关系列表：点一条就跳到对面那个节点 */}
      <div>
        <div className="text-[10px] mb-1.5" style={{ color: 'var(--text-tertiary)' }}>关系（{links.length}）</div>
        <div className="space-y-1">
          {links.length === 0 && <div className="text-[10px]" style={{ color: 'var(--text-tertiary)' }}>暂无关联边</div>}
          {links.slice(0, MAX_LINK_ROWS).map((l) => (
            <button
              key={l.id}
              onClick={() => onJump(l.other)}
              className="w-full text-left px-1.5 py-1 rounded hover:opacity-80"
              style={{ backgroundColor: 'var(--bg-tertiary)' }}
            >
              <div className="text-[10px] truncate" style={{ color: 'var(--text-primary)' }}>{l.label}</div>
              <div className="flex items-center gap-1 mt-0.5 flex-wrap">
                {l.kinds.map((k) => (
                  <span
                    key={k}
                    className="shrink-0 whitespace-nowrap text-[9px] px-1 rounded"
                    style={{ backgroundColor: `${kindColor(k)}22`, color: kindColor(k) }}
                  >
                    {kindLabel(k)}
                  </span>
                ))}
              </div>
            </button>
          ))}
          {links.length > MAX_LINK_ROWS && (
            <div className="text-[10px] px-1.5" style={{ color: 'var(--text-tertiary)' }}>
              …另有 {links.length - MAX_LINK_ROWS} 条，画布上已一并画出（可直接点那些点）
            </div>
          )}
        </div>
      </div>

      <button
        onClick={onGoEntries}
        className="pd-btn w-full flex items-center justify-center gap-1 px-2 py-1 rounded-lg text-[11px]"
        style={{ color: 'var(--text-secondary)', backgroundColor: 'var(--bg-tertiary)', border: '1px solid var(--border)' }}
      >
        <Maximize2 size={11} />
        去「知识」列表编辑
      </button>
    </div>
  );
}
