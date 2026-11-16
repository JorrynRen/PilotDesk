import { useEffect, useState } from 'react';
import { Check, KeyRound, Loader2, Server, ShieldCheck } from 'lucide-react';
import { ConversionDialogShell } from '../common/ConversionDialogShell';
import { useApiProviderStore } from '../../stores/apiProviderStore';
import { useEnclaveApiShareStore } from '../../stores/enclaveApiShareStore';
import { showToast } from '../../utils/toast';
import type {
  ApiShareDistributionView,
  OpenedApiShareConnection,
} from '../../types/enclave';

interface Props {
  orgId: string;
  distribution: Pick<
    ApiShareDistributionView,
    'id' | 'kind' | 'sourceName' | 'models' | 'expiresAt'
  >;
  onClose: () => void;
  onImported: () => void;
}

export function ApiShareConfigImportDialog({
  orgId,
  distribution,
  onClose,
  onImported,
}: Props) {
  const [connection, setConnection] = useState<OpenedApiShareConnection | null>(null);
  const [loading, setLoading] = useState(true);
  const [importing, setImporting] = useState(false);
  const [error, setError] = useState('');
  const openDistribution = useEnclaveApiShareStore((state) => state.openDistribution);
  const markDistributionApplied = useEnclaveApiShareStore(
    (state) => state.markDistributionApplied,
  );
  const saveProvider = useApiProviderStore((state) => state.saveProvider);
  const routeConnection = distribution.kind === 'route_token';

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      const opened = await openDistribution(orgId, distribution.id);
      if (cancelled) return;
      if (opened) {
        setConnection(opened);
      } else {
        setError(useEnclaveApiShareStore.getState().error ?? '无法读取共享连接');
      }
      setLoading(false);
    })();
    return () => {
      cancelled = true;
    };
  }, [distribution.id, openDistribution, orgId]);

  const importProvider = async () => {
    if (!connection) return;
    setImporting(true);
    try {
      await saveProvider({
        id: connection.providerId,
        name: connection.providerName || connection.sourceName,
        apiEndpoint: connection.baseUrl,
        apiKey: connection.apiKey,
        models: connection.models,
        modelNotes: connection.modelNotes,
        apiFormat: connection.apiFormat || 'openai',
        trustDomain: 'cloud',
        sortOrder: Date.now(),
      });
      const marked = await markDistributionApplied(orgId, distribution.id);
      showToast(
        marked ? '共享连接已添加到本机' : '连接已添加，但分发状态更新失败',
        marked ? 'success' : 'warning',
      );
      onImported();
    } catch {
      setError(useApiProviderStore.getState().error ?? '添加 Provider 失败');
    } finally {
      setImporting(false);
    }
  };

  return (
    <ConversionDialogShell
      icon={routeConnection ? <KeyRound size={17} /> : <Server size={17} />}
      kicker={routeConnection ? '路由令牌' : 'Provider 连接'}
      title="添加共享连接"
      description={routeConnection
        ? '通过飞地 HTTPS 网关访问共享模型，连接令牌不会暴露 Provider Key。'
        : '从团队主机读取一次性连接信息，确认后保存为本机独立 Provider。'}
      busy={loading || importing}
      onClose={onClose}
      footer={(
        <>
          <span className="flex-1" />
          <button type="button" className="pd-btn" onClick={onClose} disabled={importing}>
            取消
          </button>
          <button
            type="button"
            className="pd-btn pd-btn-primary"
            onClick={() => void importProvider()}
            disabled={!connection || importing}
          >
            {importing ? <Loader2 size={12} className="animate-spin" /> : <Check size={12} />}
            添加到本机
          </button>
        </>
      )}
    >
      {loading ? (
        <div className="enclave-api-loading">
          <Loader2 size={16} className="animate-spin" />
          正在读取共享连接
        </div>
      ) : error ? (
        <div className="enclave-api-dialog-callout is-warning">
          <ShieldCheck size={14} />
          <span>{error}</span>
        </div>
      ) : connection ? (
        <>
          <div className="enclave-api-source-summary">
            <span
              className="enclave-api-source-mark"
              data-mode={routeConnection ? 'route' : 'shared'}
            >
              {routeConnection ? <KeyRound size={14} /> : <Server size={14} />}
            </span>
            <div>
              <strong>{connection.sourceName}</strong>
              <span>{connection.providerName || connection.sourceName} · {connection.apiFormat || 'openai'}</span>
            </div>
          </div>
          <div className="enclave-api-config-preview">
            <div>
              <span>接口地址</span>
              <code title={connection.baseUrl}>{connection.baseUrl || '未配置'}</code>
            </div>
            <div>
              <span>连接方式</span>
              <strong>{routeConnection ? '飞地安全路由' : 'Provider 配置'}</strong>
            </div>
            <div>
              <span>模型</span>
              <strong title={connection.models.join(', ')}>
                {connection.models.join(' · ') || '跟随 Provider'}
              </strong>
            </div>
          </div>
          <div className="enclave-api-dialog-callout">
            <ShieldCheck size={14} />
            <span>
              {routeConnection
                ? '令牌只写入本机 Provider 凭据，不会进入团队数据库或传讯消息。'
                : 'Provider Key 只用于本次添加到本机凭据库，不会写入团队数据库或传讯消息。'}
            </span>
          </div>
        </>
      ) : null}
    </ConversionDialogShell>
  );
}
