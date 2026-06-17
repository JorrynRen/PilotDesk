import React from 'react';

/**
 * DefaultPluginPanel — 插件面板默认组件
 *
 * 当插件未提供自定义前端组件时，使用此默认面板展示插件信息。
 */
export function DefaultPluginPanel({ pluginId }: { pluginId: string }) {
  const [panelContent, setPanelContent] = React.useState<string>('加载中...');
  const [panelError, setPanelError] = React.useState<string | null>(null);

  React.useEffect(() => {
    import('@tauri-apps/api/core').then(({ invoke }) => {
      invoke<string>('plugin_get_panel_content', { pluginId, panelId: 'default' })
        .then((result) => setPanelContent(result))
        .catch((err) => {
          setPanelError(String(err));
          setPanelContent('');
        });
    });
  }, [pluginId]);

  if (panelError) {
    return React.createElement('div',
      { className: 'text-xs py-4 text-center', style: { color: '#EF4444' } },
      React.createElement('p', null, '面板加载失败'),
      React.createElement('p', { className: 'text-[10px] mt-1' }, panelError)
    );
  }

  return React.createElement('div',
    { className: 'text-xs', style: { color: 'var(--text-primary)' } },
    React.createElement('div',
      { className: 'mb-2 text-[10px]', style: { color: 'var(--text-tertiary)' } },
      '插件面板 - ', pluginId
    ),
    React.createElement('div',
      { className: 'p-3 rounded', style: { backgroundColor: 'var(--bg-tertiary)' } },
      React.createElement('pre',
        { className: 'text-[10px] whitespace-pre-wrap break-all', style: { color: 'var(--text-secondary)' } },
        panelContent || '面板内容为空'
      )
    )
  );
}
