export function MainPanel() {
  return (
    <main
      className="flex-1 flex flex-col overflow-hidden"
      style={{ backgroundColor: 'var(--bg-primary)' }}
    >
      {/* Chat Messages Area */}
      <div className="flex-1 flex items-center justify-center">
        <div className="text-center px-8">
          <div
            className="w-14 h-14 mx-auto mb-4 rounded-2xl flex items-center justify-center"
            style={{ background: 'linear-gradient(135deg, #5B7FFF, #8B5CF6)' }}
          >
            <span className="text-2xl text-white">PD</span>
          </div>
          <h2 className="text-lg font-semibold mb-1">PilotDesk</h2>
          <p className="text-sm mb-1" style={{ color: 'var(--text-secondary)' }}>
            Claude Code & Hermes Agent 统一桌面客户端
          </p>
          <p className="text-xs" style={{ color: 'var(--text-secondary)' }}>
            从左侧选择或创建会话开始对话
          </p>
        </div>
      </div>

      {/* Input Area */}
      <div className="p-4" style={{ borderTop: '1px solid var(--border)' }}>
        <div
          className="rounded-lg px-4 py-2.5 text-sm"
          style={{
            backgroundColor: 'var(--bg-secondary)',
            border: '1px solid var(--border)',
            color: 'var(--text-primary)',
            minHeight: '40px',
          }}
          contentEditable
          suppressContentEditableWarning
          data-placeholder="输入消息..."
        />
      </div>
    </main>
  );
}
