import { useInspirationStore } from '../../stores/inspirationStore';
import { TagFilterBar } from './TagFilterBar';

/**
 * TagFilter — 独立「灵感库」页的标签筛选条。
 *
 * 现在只是一层"接 store"的薄壳：独立页的筛选走 store 的 activeTag（它会触发后端按 tag 过滤），
 * 渲染交给受控组件 TagFilterBar。会话侧栏要用同一套外观、但持有自己的筛选状态，
 * 所以把受控部分抽出去，这里只负责接线。
 */
export function TagFilter() {
  const { tags, activeTag, setActiveTag } = useInspirationStore();
  return <TagFilterBar tags={tags} activeTag={activeTag} onSelect={setActiveTag} />;
}
