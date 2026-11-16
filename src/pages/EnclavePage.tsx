import { WorkbenchAssetPage } from '../components/layout';
import { TaskWorkspace } from '../components/enclave/TaskWorkspace';

/**
 * 飞地（Enclave）任务工作台 —— 独立路由 `/enclave`。
 *
 * 主链：组织 → 任务与产出 → 自动沉淀进组织知识库 → Agent 可检索。
 * 组织与成员治理在 `/enclave/org`；数据出境闸门 / 应急 / 审计也在那里。
 */
export function EnclavePage() {
  return (
    <WorkbenchAssetPage contextLabel="飞地" activeAsset="enclave" frameless>
      <div className="h-full flex flex-col overflow-hidden">
        <TaskWorkspace />
      </div>
    </WorkbenchAssetPage>
  );
}
