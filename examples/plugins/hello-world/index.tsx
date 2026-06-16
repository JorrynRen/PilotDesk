/**
 * Hello World 示例插件
 *
 * 展示插件系统的基本用法：
 * 1. 注册面板（ui:panel 权限）
 * 2. 显示通知（ui:toast 权限 - 默认授权）
 * 3. 读取会话（session:read 权限）
 * 4. 使用插件独立存储（storage:* 权限 - 默认授权）
 */

import type { PluginAPI } from '@pilotdesk/plugin-api';

// 面板组件
function HelloPanel() {
  const [count, setCount] = React.useState(0);
  const [time, setTime] = React.useState(new Date().toLocaleTimeString());

  React.useEffect(() => {
    const timer = setInterval(() => {
      setTime(new Date().toLocaleTimeString());
    }, 1000);
    return () => clearInterval(timer);
  }, []);

  return (
    <div className="hello-panel">
      <h3>Hello from Plugin!</h3>
      <p>Current time: {time}</p>
      <p>Button clicked: {count} times</p>
      <button onClick={() => setCount(c => c + 1)}>
        Click me
      </button>
    </div>
  );
}

// 插件入口
export default {
  onLoad(api: PluginAPI) {
    console.log('[HelloWorld] Plugin loaded');

    // 注册面板
    api.ui.addPanel({
      id: 'hello-panel',
      title: 'Hello World',
      component: HelloPanel,
    });

    // 显示通知
    api.ui.showToast('Hello World 插件已加载', 'success');

    // 监听事件
    const unlisten = api.events.on('message:before-send', (message) => {
      console.log('[HelloWorld] Message about to be sent:', message);
    });

    // 保存到实例以便卸载时清理
    (window as any).__helloWorldUnlisten = unlisten;
  },

  onUnload() {
    console.log('[HelloWorld] Plugin unloaded');

    // 清理事件监听
    if ((window as any).__helloWorldUnlisten) {
      (window as any).__helloWorldUnlisten();
      delete (window as any).__helloWorldUnlisten;
    }
  },
};
