import { Component, type ReactNode, type ErrorInfo } from 'react';

interface Props {
  children: ReactNode;
  fallback?: ReactNode;
}

interface State {
  hasError: boolean;
  error: Error | null;
}

export class ErrorBoundary extends Component<Props, State> {
  public state: State = {
    hasError: false,
    error: null,
  };

  public static getDerivedStateFromError(error: Error): State {
    return { hasError: true, error };
  }

  public componentDidCatch(error: Error, errorInfo: ErrorInfo) {
    console.error('[ErrorBoundary] React rendering error:', error, errorInfo);
  }

  private handleReset = () => {
    this.setState({ hasError: false, error: null });
  };

  public render() {
    if (this.state.hasError) {
      if (this.props.fallback) {
        return this.props.fallback;
      }

      return (
        <div
          style={{
            position: 'fixed',
            inset: 0,
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            flexDirection: 'column',
            gap: '12px',
            backgroundColor: 'var(--bg-primary)',
            color: 'var(--text-primary)',
            fontFamily: "'Segoe UI', -apple-system, sans-serif",
            padding: '24px',
            zIndex: 99999,
          }}
        >
          <div style={{ fontSize: '14px', fontWeight: 600 }}>PilotDesk 渲染出错</div>
          <div
            style={{
              fontSize: '12px',
              color: 'var(--text-secondary)',
              textAlign: 'center',
              maxWidth: '600px',
              wordBreak: 'break-word',
            }}
          >
            {this.state.error?.message || '未知错误'}
          </div>
          {this.state.error?.stack && (
            <pre
              style={{
                fontSize: '10px',
                color: 'var(--text-tertiary)',
                backgroundColor: 'var(--bg-tertiary)',
                padding: '8px',
                borderRadius: '6px',
                maxHeight: '200px',
                overflow: 'auto',
                width: '100%',
                maxWidth: '600px',
                whiteSpace: 'pre-wrap',
              }}
            >
              {this.state.error.stack}
            </pre>
          )}
          <button
            onClick={handleReset}
            style={{
              marginTop: '8px',
              padding: '6px 16px',
              borderRadius: '6px',
              fontSize: '12px',
              border: '1px solid var(--border)',
              backgroundColor: 'var(--accent)',
              color: '#fff',
              cursor: 'pointer',
            }}
          >
            重试
          </button>
        </div>
      );
    }

    return this.props.children;
  }
}
