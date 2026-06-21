/**
 * Hello World 示例插件
 *
 * 展示插件系统的基本用法：
 * 1. 注册面板（ui:panel 权限）
 * 2. 显示通知（ui:toast 权限 - 默认授权）
 * 3. 读取会话（session:read 权限）
 * 4. 使用插件独立存储（storage:* 权限 - 默认授权）
 *
 * 注意：入口文件使用纯 JS 格式（React.createElement 替代 JSX）
 */

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
  onLoad: function(api) {
    console.log('[HelloWorld] Plugin loaded');

    // 注册面板（覆盖 manifest.json contributes 中的默认面板）
    api.ui.addPanel({
      id: 'hello-panel',
      title: 'Hello World',
      component: HelloPanel,
    });

    // 显示通知
    api.ui.showToast('Hello World 插件已加载', 'success');

    // 监听事件
    var unlisten = api.events.on('message:before-send', function(message) {
      console.log('[HelloWorld] Message about to be sent:', message);
    });

    window.__helloWorldUnlisten = unlisten;
  },

  onUnload: function() {
    console.log('[HelloWorld] Plugin unloaded');

    if (window.__helloWorldUnlisten) {
      window.__helloWorldUnlisten();
      delete window.__helloWorldUnlisten;
    }
  },
};
