/**
 * Hello World 示例插件
 *
 * 展示插件系统的基本用法：
 * 1. 注册面板（需在 permissions 里声明 `ui:panel`）
 * 2. 注册命令处理函数 —— `contributes.commands` 只负责"声明"，实现在这里
 * 3. 显示通知（`ui:toast` 属默认授权，无需声明）
 * 4. 监听应用事件（两种写法：声明式 contributes.hooks / 命令式 api.events.on）
 *
 * 三个注意点：
 * - 入口用纯 JS（用 React.createElement 代替 JSX）
 * - 运行环境遮蔽了 window / globalThis / document / fetch 等危险全局：
 *   不要依赖它们；跨生命周期共享状态请用**模块作用域变量**
 * - 应用事件是**仅通知**：handler 的返回值/异常都不影响应用，没有"发送前拦截"能力
 */

// 模块作用域：onLoad 里注册的退订函数，onUnload 里清理
var offMessageSent = null;

// 面板组件（使用 React.createElement 替代 JSX）
function HelloPanel(props) {
  var count = React.useState(0);
  var time = React.useState(new Date().toLocaleTimeString());
  var setCount = count[1];
  var setTime = time[1];
  var countVal = count[0];
  var timeVal = time[0];

  React.useEffect(function() {
    var timer = setInterval(function() {
      setTime(new Date().toLocaleTimeString());
    }, 1000);
    return function() { clearInterval(timer); };
  }, []);

  return React.createElement('div', { className: 'hello-panel' },
    React.createElement('h3', null, 'Hello from Plugin!'),
    React.createElement('p', null, 'Current time: ', timeVal),
    React.createElement('p', null, 'Button clicked: ', countVal, ' times'),
    React.createElement('button', { onClick: function() { setCount(countVal + 1); } }, 'Click me')
  );
}

// 插件入口
export default {
  // 声明式事件钩子：manifest.contributes.hooks 里的
  // { "event": "session:created", "handler": "onSessionCreated" } 指向本函数；
  // 宿主加载插件时会把它接到应用事件总线（与 api.events.on 同一实现）。
  onSessionCreated: function(payload) {
    console.log('[HelloWorld] 新会话已创建:', payload);
  },

  onLoad: function(api) {
    console.log('[HelloWorld] Plugin loaded');

    // 1. 注册面板（覆盖 manifest.json contributes 中声明的面板）
    api.ui.addPanel({
      id: 'hello-panel',
      title: 'Hello World',
      component: HelloPanel,
    });

    // 2. 注册命令处理函数。
    //    manifest.json 的 contributes.commands 只是**声明**（工作流节点/命令面板据此列出，
    //    并用其中的 input 模式生成参数表单）；真正的实现在这里。
    //    只声明不注册的话，调用时会返回「命令未注册」。
    //    handler 必须返回 Promise（用 async），返回值需可 JSON 序列化。
    api.commands.register('hello.say', async function(params) {
      var who = params && typeof params.name === 'string' && params.name ? params.name : 'World';
      return { message: 'Hello, ' + who + '!' };
    });

    api.commands.register('hello.time', async function() {
      return { time: new Date().toLocaleTimeString() };
    });

    // 3. 命令式监听应用事件：api.events.on 订阅宿主事件总线，返回退订函数。
    //    载荷只带标识（sessionId / messageId / role），不含消息正文。
    //    宿主在插件卸载时也会自动清理，这里显式退订只是演示如何手动收尾。
    offMessageSent = api.events.on('message:sent', function(payload) {
      if (payload && payload.role === 'user') {
        console.log('[HelloWorld] 用户发出消息:', payload);
      }
    });

    // 4. 显示通知
    api.ui.showToast('Hello World 插件已加载', 'success');
  },

  onUnload: function() {
    // 命令、面板组件、事件订阅由宿主在卸载时自动清理；这里演示显式退订。
    // （若你在模块作用域里起了 setInterval 之类的东西，记得在这里清掉。）
    if (offMessageSent) {
      offMessageSent();
      offMessageSent = null;
    }
    console.log('[HelloWorld] Plugin unloaded');
  },
};
