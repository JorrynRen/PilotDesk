import type { LucideIcon } from 'lucide-react';
import { Shield, ShieldCheck, ShieldAlert, AlertTriangle } from 'lucide-react';
import type { SecurityModeValue } from './SecurityModeSelector';

// ============================================================
// 会话安全模式清单
// - 与 SecurityModeSelector 分离：该文件含有组件，常量导出会触发
//   react-refresh/only-export-components，故独立成本模块。
// - 说明按"审批尺度"口径写：模式越严，越低风险的调用也要确认（命令/路径
//   另按各自的子策略矩阵分级，见 agent_loop.rs 的 command_action/path_action）
// ============================================================
export const SECURITY_MODES: {
  value: SecurityModeValue;
  name: string;
  desc: string;
  icon: LucideIcon;
  color: string;
  danger?: boolean;
}[] = [
  {
    value: 'strict',
    name: '严格模式',
    desc: '所有命令、路径与工具调用都要确认',
    icon: ShieldAlert,
    color: '#F59E0B',
  },
  {
    value: 'standard',
    name: '标准模式',
    desc: '风险/未知命令与路径、中高风险工具需确认',
    icon: ShieldCheck,
    color: 'var(--accent)',
  },
  {
    value: 'relaxed',
    name: '宽松模式',
    desc: '仅高风险操作需确认，其余放行',
    icon: Shield,
    color: 'var(--text-secondary)',
  },
  {
    value: 'unrestricted',
    name: '无限制模式',
    desc: '全部放行、不做任何确认，高风险',
    icon: AlertTriangle,
    color: 'var(--status-danger)',
    danger: true,
  },
];
