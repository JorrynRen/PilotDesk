/**
 * TabIcon — 门户标签图标渲染（顶栏段 / 设置页 / 门户卡片三处共用）。
 *
 * 图标以 key 持久化（`CustomTab.icon`），key 取自 store 的有限集合 `CUSTOM_TAB_ICONS`；
 * 这里把 key 映射为 lucide 组件，非法 / 缺失一律回落地球图标 —— 避免旧数据或脏数据把图标渲染成空白。
 */
import { Globe, Link2, Book, Briefcase, Code, BarChart3, Mail, Calendar, FileText, Star, type LucideIcon } from 'lucide-react';
import { normalizeTabIcon } from '../../stores/customTabsStore';

/** key → lucide 图标（与 store 的 CUSTOM_TAB_ICONS 一一对应） */
const ICON_MAP: Record<string, LucideIcon> = {
  globe: Globe,
  link: Link2,
  book: Book,
  briefcase: Briefcase,
  code: Code,
  chart: BarChart3,
  mail: Mail,
  calendar: Calendar,
  file: FileText,
  star: Star,
};

/**
 * 按引用渲染一个图标组件。
 * 单独抽一层是为了把「组件引用」作为 props 传入后再渲染 —— 仓库既有写法（见 KnowledgeGraph 的 BlockTitle），
 * 避免在渲染期用局部变量拼装组件而触发 react-hooks/static-components。
 */
function IconByRef({ icon: Icon, size, className }: { icon: LucideIcon; size: number; className?: string }) {
  return <Icon size={size} className={className} />;
}

export function TabIcon({ icon, size = 14, className }: { icon?: string; size?: number; className?: string }) {
  return <IconByRef icon={ICON_MAP[normalizeTabIcon(icon) ?? ''] ?? Globe} size={size} className={className} />;
}