import { useSyncExternalStore } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useEnclaveOrgStore } from '../stores/enclaveOrgStore';
import { useKnowledgeStore } from '../stores/knowledgeStore';
import type { OrgSnapshotView } from '../types/enclave';

interface PendingTaskSummary {
  assignedPending: number;
  awaitingReview: number;
  total: number;
}

export interface WorkbenchPendingIndicators {
  knowledge: number;
  enclave: number;
  enclaveAssigned: number;
  enclaveReview: number;
}

const EMPTY: WorkbenchPendingIndicators = {
  knowledge: 0,
  enclave: 0,
  enclaveAssigned: 0,
  enclaveReview: 0,
};

let snapshot = EMPTY;
const subscribers = new Set<() => void>();
let refreshInFlight: Promise<void> | null = null;
let intervalId: number | null = null;
let stopEnclaveEvents: (() => void) | null = null;
let stopOrgSubscription: (() => void) | null = null;
let stopKnowledgeSubscription: (() => void) | null = null;
let focusHandler: (() => void) | null = null;
let enclaveRefreshTimer: number | null = null;

function emit(next: WorkbenchPendingIndicators) {
  if (
    next.knowledge === snapshot.knowledge
    && next.enclave === snapshot.enclave
    && next.enclaveAssigned === snapshot.enclaveAssigned
    && next.enclaveReview === snapshot.enclaveReview
  ) return;
  snapshot = next;
  subscribers.forEach((listener) => listener());
}

async function loadKnowledgePending(): Promise<number> {
  const bases = await invoke<{ id: string; pendingCount: number }[]>('kb_list_bases');
  const counts = Object.fromEntries(bases.map((base) => [base.id, base.pendingCount ?? 0]));
  useKnowledgeStore.getState().syncPendingCounts(counts);
  return bases.reduce((total, base) => total + (base.pendingCount ?? 0), 0);
}

function knowledgePendingFromStore(): number {
  return useKnowledgeStore
    .getState()
    .bases
    .reduce((total, base) => total + (base.pendingCount ?? 0), 0);
}

async function loadEnclavePending(): Promise<Pick<
  WorkbenchPendingIndicators,
  'enclave' | 'enclaveAssigned' | 'enclaveReview'
>> {
  const orgs = await invoke<OrgSnapshotView[]>('enclave_org_list');
  if (orgs.length === 0) {
    return { enclave: 0, enclaveAssigned: 0, enclaveReview: 0 };
  }
  const activeOrgId = useEnclaveOrgStore.getState().activeOrgId;
  const orgId = orgs.some((org) => org.orgId === activeOrgId)
    ? activeOrgId
    : orgs[0].orgId;
  const summary = await invoke<PendingTaskSummary>('enclave_task_pending_summary', { orgId });
  return {
    enclave: summary.total,
    enclaveAssigned: summary.assignedPending,
    enclaveReview: summary.awaitingReview,
  };
}

export function refreshWorkbenchPendingIndicators(): Promise<void> {
  if (refreshInFlight) return refreshInFlight;
  refreshInFlight = (async () => {
    const [knowledge, enclave] = await Promise.allSettled([
      loadKnowledgePending(),
      loadEnclavePending(),
    ]);
    emit({
      knowledge: knowledge.status === 'fulfilled' ? knowledge.value : snapshot.knowledge,
      enclave: enclave.status === 'fulfilled' ? enclave.value.enclave : snapshot.enclave,
      enclaveAssigned: enclave.status === 'fulfilled'
        ? enclave.value.enclaveAssigned
        : snapshot.enclaveAssigned,
      enclaveReview: enclave.status === 'fulfilled'
        ? enclave.value.enclaveReview
        : snapshot.enclaveReview,
    });
  })().finally(() => {
    refreshInFlight = null;
  });
  return refreshInFlight;
}

function start() {
  if (intervalId !== null) return;
  void refreshWorkbenchPendingIndicators();
  intervalId = window.setInterval(
    () => void refreshWorkbenchPendingIndicators(),
    30_000,
  );
  focusHandler = () => void refreshWorkbenchPendingIndicators();
  window.addEventListener('focus', focusHandler);

  emit({ ...snapshot, knowledge: knowledgePendingFromStore() });
  stopKnowledgeSubscription = useKnowledgeStore.subscribe((state, previous) => {
    if (state.bases !== previous.bases) {
      emit({ ...snapshot, knowledge: knowledgePendingFromStore() });
    }
  });

  stopOrgSubscription = useEnclaveOrgStore.subscribe((state, previous) => {
    if (state.activeOrgId !== previous.activeOrgId) {
      void refreshWorkbenchPendingIndicators();
    }
  });

  let disposed = false;
  let unlisten: (() => void) | null = null;
  void listen<{ type?: string }>('enclave:changed', (event) => {
    if (event.payload?.type === 'realtime') return;
    if (enclaveRefreshTimer !== null) window.clearTimeout(enclaveRefreshTimer);
    enclaveRefreshTimer = window.setTimeout(() => {
      enclaveRefreshTimer = null;
      void refreshWorkbenchPendingIndicators();
    }, 500);
  }).then((fn) => {
    if (disposed) fn();
    else unlisten = fn;
  });

  stopEnclaveEvents = () => {
    disposed = true;
    unlisten?.();
    if (enclaveRefreshTimer !== null) {
      window.clearTimeout(enclaveRefreshTimer);
      enclaveRefreshTimer = null;
    }
  };
}

function stop() {
  if (intervalId !== null) {
    window.clearInterval(intervalId);
    intervalId = null;
  }
  if (focusHandler) {
    window.removeEventListener('focus', focusHandler);
    focusHandler = null;
  }
  stopOrgSubscription?.();
  stopOrgSubscription = null;
  stopKnowledgeSubscription?.();
  stopKnowledgeSubscription = null;
  stopEnclaveEvents?.();
  stopEnclaveEvents = null;
}

function subscribe(listener: () => void) {
  subscribers.add(listener);
  if (subscribers.size === 1) start();
  return () => {
    subscribers.delete(listener);
    if (subscribers.size === 0) stop();
  };
}

function getSnapshot() {
  return snapshot;
}

export function useWorkbenchPendingIndicators(): WorkbenchPendingIndicators {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}
