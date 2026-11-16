import { useMemo } from 'react';
import { useGroupChatStore } from '../stores/groupChatStore';
import { useWorkflowStore } from '../stores/workflowStore';

const ACTIVE_WORKFLOW_STATUSES = new Set(['pending', 'running', 'paused']);
const ACTIVE_ROOM_STATUSES = new Set(['running', 'paused']);

/**
 * 顶栏与会话默认页共用的运行摘要。
 *
 * 待处理只统计仍可处理的事项；进程重启后遗留的失效审批单独返回，供完整面板收尾。
 */
export function useRuntimeSummary() {
  const instances = useWorkflowStore((s) => s.instances);
  const pendingInputs = useWorkflowStore((s) => s.pendingInputs);
  const pendingApprovals = useWorkflowStore((s) => s.pendingApprovals);
  const rooms = useGroupChatStore((s) => s.rooms);

  return useMemo(() => {
    const activeApprovals = pendingApprovals.filter((item) => !item.stale);
    const staleApprovals = pendingApprovals.filter((item) => item.stale);
    const activeInstances = instances.filter((item) => ACTIVE_WORKFLOW_STATUSES.has(item.status));
    const activeRooms = rooms.filter((item) => ACTIVE_ROOM_STATUSES.has(item.status));
    const runningWorkflowCount = instances.filter((item) => item.status === 'running').length;
    const runningRoomCount = rooms.filter((item) => item.status === 'running').length;

    return {
      instances,
      rooms,
      pendingInputs,
      activeApprovals,
      staleApprovals,
      activeInstances,
      activeRooms,
      pendingCount: activeApprovals.length + pendingInputs.length,
      activeCount: activeInstances.length + activeRooms.length,
      runningCount: runningWorkflowCount + runningRoomCount,
    };
  }, [instances, pendingApprovals, pendingInputs, rooms]);
}
