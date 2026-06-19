import { useState, useEffect, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { Terminal, Trash2 } from 'lucide-react';

interface LogEntry {
  id?: number;
  timestamp: number;
  message: string;
  level: 'info' | 'warn' | 'error' | 'success';
}

interface InstallLogProps {
  logs?: LogEntry[];
  isActive?: boolean;
  onClear?: () => void;
}

export function InstallLog({ logs: externalLogs, isActive, onClear }: InstallLogProps) {
  const [internalLogs, setInternalLogs] = useState<LogEntry[]>([]);
  const logs = externalLogs ?? internalLogs;
  const scrollRef = useRef<HTMLDivElement>(null);
  const initialized = useRef(false);

  // Auto-scroll to bottom when logs change
  useEffect(() => {
    if (scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
    }
  }, [logs]);

  // Load persisted logs from Rust on mount (only for internal mode)
  useEffect(() => {
    if (externalLogs || initialized.current) return;
    initialized.current = true;
    invoke<LogEntry[]>('list_logs', { limit: 200 })
      .then((entries) => {
        setInternalLogs(entries || []);
      })
      .catch(() => {
        // Table may not exist yet on first run, ignore
      });
  }, [externalLogs]);

  // Listen for install-progress events to dynamically refresh log list
  useEffect(() => {
    if (externalLogs) return;
    const unlisten = listen<string>('log-updated', () => {
      invoke<LogEntry[]>('list_logs', { limit: 200 })
        .then((entries) => {
          setInternalLogs(entries || []);
        })
        .catch(() => {});
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [externalLogs]);

  // Auto-scroll to bottom when new logs arrive (ascending order display)

  const levelColor: Record<string, string> = {
    info: 'var(--text-secondary)',
    warn: '#F59E0B',
    error: '#EF4444',
    success: '#10B981',
  };

  const handleClear = () => {
    if (onClear) {
      onClear();
    } else {
      // Internal mode: clear via Rust command
      invoke('clear_logs').then(() => {
        setInternalLogs([]);
      }).catch(() => {});
    }
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
          <span className="text-[10px] " style={{ color: 'var(--text-secondary)' }}>
            操作日志
          </span>
          {isActive && (
            <span className="text-[10px] px-1.5 py-0.5 rounded-full animate-pulse" style={{ backgroundColor: '#3B82F622', color: '#3B82F6' }}>
              运行中
            </span>
          )}
        </div>
        <button
          onClick={handleClear}
          className="pd-btn p-0.5 rounded transition-colors"
          style={{ color: 'var(--text-tertiary)' }}
          title="清空日志"
        >
          <Trash2 size={12} />
        </button>
      </div>

      {/* Log content */}
      <div ref={scrollRef} className="px-3 py-2 font-mono text-[11px] leading-relaxed overflow-y-auto" style={{ maxHeight: '95px' }}>
        {logs.length === 0 ? (
          <span style={{ color: 'var(--text-tertiary)' }}>等待操作...</span>
        ) : (
          logs.map((entry, idx) => {
            const time = new Date(entry.timestamp).toLocaleString('zh-CN', {
              year: 'numeric',
              month: '2-digit',
              day: '2-digit',
              hour: '2-digit',
              minute: '2-digit',
              second: '2-digit',
            });
            return (
              <div key={entry.id || idx} className="flex gap-2">
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
