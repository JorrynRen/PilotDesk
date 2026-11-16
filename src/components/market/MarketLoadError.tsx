import { AlertCircle, RefreshCw } from 'lucide-react';

interface MarketLoadErrorProps {
  error: string;
  onRetry: () => void;
  fallbackTitle?: string;
  className?: string;
}

const CONNECTION_ERROR_HINTS = [
  '连接失败',
  '连接超时',
  '请求超时',
  '无法连接',
  '网络连接失败',
  '网络错误',
  'network error',
  'failed to connect',
  'error sending request',
  'connection refused',
  'connection reset',
  'fetch failed',
  'timed out',
  'timeout',
  'unreachable',
  'econnrefused',
  'econnreset',
  'enotfound',
  'etimedout',
];

function errorCopy(error: string, fallbackTitle: string): { title: string; detail: string } {
  const normalized = error.toLowerCase();
  if (CONNECTION_ERROR_HINTS.some((hint) => normalized.includes(hint))) {
    return {
      title: '无法连接服务器',
      detail: '请检查网络连接后重试。',
    };
  }
  return {
    title: fallbackTitle,
    detail: '市场服务暂时不可用，请稍后重试。',
  };
}

export function MarketLoadError({
  error,
  onRetry,
  fallbackTitle = '市场加载失败',
  className = '',
}: MarketLoadErrorProps) {
  const copy = errorCopy(error, fallbackTitle);

  return (
    <div
      role="alert"
      className={`flex items-center gap-3 rounded-lg px-3 py-2.5 ${className}`}
      style={{
        backgroundColor: 'var(--status-danger-bg)',
        border: '1px solid color-mix(in srgb, var(--status-danger) 24%, var(--border))',
      }}
      title={error}
    >
      <AlertCircle
        size={14}
        className="shrink-0"
        style={{ color: 'var(--status-danger)' }}
      />
      <div className="min-w-0 flex-1">
        <div className="text-[11px] font-medium" style={{ color: 'var(--status-danger)' }}>
          {copy.title}
        </div>
        <div className="mt-0.5 text-[10px] leading-4" style={{ color: 'var(--text-secondary)' }}>
          {copy.detail}
        </div>
      </div>
      <button
        type="button"
        onClick={onRetry}
        className="pd-btn inline-flex shrink-0 items-center gap-1 rounded-md px-2.5 text-[10px]"
        style={{
          height: 26,
          backgroundColor: 'var(--bg-field)',
          border: '1px solid var(--border)',
          color: 'var(--text-primary)',
        }}
      >
        <RefreshCw size={10} />
        重试
      </button>
    </div>
  );
}
