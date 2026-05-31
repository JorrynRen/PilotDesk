import { useState, useEffect, useRef } from 'react';
import { Terminal, Trash2 } from 'lucide-react';

interface LogEntry {
  timestamp: number;
  message: string;
  level: 'info' | 'warn' | 'error' | 'success';
}

interface InstallLogProps {
  logs?: LogEntry[];
  isActive?: boolean;
}

export function InstallLog({ logs: externalLogs, isActive }: InstallLogProps) {
  const [internalLogs, setInternalLogs] = useState<LogEntry[]>([]);
  const logs = externalLogs ?? internalLogs;
  const scrollRef = useRef<HTMLDivElement>(null);

  // Auto-scroll to bottom
  useEffect(() => {
    if (scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
    }
  }, [logs]);

  const levelColor: Record<string, string> = {
    info: 'var(--text-secondary)',
    warn: '#F59E0B',
    error: '#EF4444',
    success: '#10B981',
  };

  return (
    <div
      className="rounded-lg overflow-hidden"
      style={{
        border: '1px solid var(--border)',
        backgroundColor: 'var(--bg-tertiary)',
      }}
    >
      {/* Header */}
      <div className="flex items-center justify-between px-3 py-1.5" style={{ borderBottom: '1px solid var(--border)' }}>
        <div className="flex items-center gap-1.5">
          <Terminal size={12} style={{ color: 'var(--text-tertiary)' }} />
          <span className="text-[10px] font-medium" style={{ color: 'var(--text-secondary)' }}>
            安装日志
          </span>
          {isActive && (
            <span className="text-[10px] px-1.5 py-0.5 rounded-full animate-pulse" style={{ backgroundColor: '#3B82F622', color: '#3B82F6' }}>
              运行中
            </span>
          )}
        </div>
        <button
          onClick={() => setInternalLogs([])}
          className="p-0.5 rounded"
          style={{ color: 'var(--text-tertiary)' }}
          title="清空日志"
        >
          <Trash2 size={12} />
        </button>
      </div>

      {/* Log content */}
      <div ref={scrollRef} className="px-3 py-2 font-mono text-[11px] leading-relaxed overflow-y-auto" style={{ maxHeight: '160px' }}>
        {logs.length === 0 ? (
          <span style={{ color: 'var(--text-tertiary)' }}>等待操作...</span>
        ) : (
          logs.map((entry, idx) => {
            const time = new Date(entry.timestamp).toLocaleTimeString('zh-CN', {
              hour: '2-digit',
              minute: '2-digit',
              second: '2-digit',
            });
            return (
              <div key={idx} className="flex gap-2">
                <span style={{ color: 'var(--text-tertiary)', whiteSpace: 'nowrap' }}>{time}</span>
                <span style={{ color: levelColor[entry.level] ?? 'var(--text-secondary)' }}>
                  {entry.message}
                </span>
              </div>
            );
          })
        )}
      </div>
    </div>
  );
}
