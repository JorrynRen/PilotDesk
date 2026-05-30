export function StatusBar() {
  return (
    <footer
      className="flex items-center justify-between px-4 h-6 text-xs shrink-0 select-none"
      style={{ borderTop: '1px solid var(--border)', color: 'var(--text-secondary)', backgroundColor: 'var(--bg-secondary)' }}
    >
      <div className="flex items-center gap-3">
        <span className="flex items-center gap-1">
          <span className="w-1.5 h-1.5 rounded-full bg-green-500" />
          就绪
        </span>
        <span>Claude Code: --</span>
        <span>Hermes: --</span>
      </div>
      <div className="flex items-center gap-3">
        <span>Node: --</span>
        <span>Git: --</span>
      </div>
    </footer>
  );
}
