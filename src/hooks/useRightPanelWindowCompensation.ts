import { useEffect, useRef } from 'react';
import {
  currentMonitor,
  getCurrentWindow,
  LogicalSize,
} from '@tauri-apps/api/window';

function getPendingOverflow(): number {
  const containers = Array.from(document.querySelectorAll<HTMLElement>(
    '.workbench-body, .workbench-workspace, .workbench-groupchat-shell',
  ));
  const centerMinWidth = Number.parseFloat(
    getComputedStyle(document.documentElement).getPropertyValue('--workbench-center-min-width'),
  ) || 0;
  const leftPanelWidth = Number.parseFloat(
    getComputedStyle(document.documentElement).getPropertyValue('--workbench-left-panel-width'),
  ) || 0;
  const centerDeficit = Math.max(
    0,
    ...Array.from(document.querySelectorAll<HTMLElement>(
      '.pd-main-panel, .workbench-center-surface',
    )).map((element) => {
      if (!element || element.getClientRects().length === 0) return 0;
      const requiredWidth = element.classList.contains('workbench-terminal-surface')
        ? leftPanelWidth + centerMinWidth
        : centerMinWidth;
      return requiredWidth - element.getBoundingClientRect().width;
    }),
  );

  return Math.ceil(Math.max(
    0,
    centerDeficit,
    document.documentElement.scrollWidth - document.documentElement.clientWidth,
    ...containers.map((element) => {
      if (!element || element.getClientRects().length === 0) return 0;
      return element.scrollWidth - element.clientWidth;
    }),
  ));
}

function waitForLayout(): Promise<void> {
  return new Promise((resolve) => {
    requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
  });
}

type RightPanelMode = 'session' | 'terminal' | 'groupchat';

interface CompensationState {
  mode: RightPanelMode | null;
  open: boolean;
  baseWidth: number | null;
  appliedDelta: number;
  syncing: boolean;
  needsSync: boolean;
  retryAttempts: number;
  retryTimer: number | null;
  disposed: boolean;
}

async function syncWindow(state: CompensationState): Promise<void> {
  if (state.disposed) return;
  if (state.syncing) {
    state.needsSync = true;
    return;
  }
  state.syncing = true;

  try {
    if (state.retryTimer !== null) {
      window.clearTimeout(state.retryTimer);
      state.retryTimer = null;
    }

    const active = state.open && state.mode !== null;
    if (!active && state.baseWidth === null && state.appliedDelta <= 0) {
      state.retryAttempts = 0;
      return;
    }

    const win = getCurrentWindow();
    const [maximized, fullscreen] = await Promise.all([
      win.isMaximized(),
      win.isFullscreen(),
    ]);
    const constrained = maximized || fullscreen;

    if (active) {
      if (constrained) return;

      const scale = await win.scaleFactor();
      const initial = (await win.innerSize()).toLogical(scale);
      let maxWidth = Number.POSITIVE_INFINITY;
      try {
        const monitor = await currentMonitor();
        if (monitor) {
          maxWidth = monitor.size.width / (monitor.scaleFactor || scale);
        }
      } catch {
        /* Monitor lookup is optional; the OS still applies its own bounds. */
      }

      if (state.baseWidth === null) state.baseWidth = initial.width;
      // Breakpoints and lazy pages can change the required width after the first
      // resize, so measure and settle any newly exposed overflow in up to 3 passes.
      for (let pass = 0; pass < 3; pass += 1) {
        await waitForLayout();
        const overflow = getPendingOverflow();
        if (overflow <= 0) break;

        const current = (await win.innerSize()).toLogical(scale);
        const targetWidth = Math.min(
          current.width + overflow,
          maxWidth,
        );
        const delta = targetWidth - current.width;
        if (delta < 1) break;

        state.appliedDelta += delta;
        await win.setSize(new LogicalSize(targetWidth, current.height));
      }
      state.retryAttempts = 0;
      return;
    }

    if (constrained) {
      if (state.appliedDelta <= 0) state.baseWidth = null;
      return;
    }

    const baseWidth = state.baseWidth;
    if (baseWidth === null || state.appliedDelta <= 0) {
      state.baseWidth = null;
      state.appliedDelta = 0;
      return;
    }

    const scale = await win.scaleFactor();
    const current = (await win.innerSize()).toLogical(scale);
    await win.setSize(new LogicalSize(baseWidth, current.height));
    state.baseWidth = null;
    state.appliedDelta = 0;
    state.retryAttempts = 0;
  } catch {
    // A dev-server restart can briefly break Tauri's IPC transport. Retry a
    // bounded number of times; plain browser previews skip this entirely.
    const canRetry = '__TAURI_INTERNALS__' in window && state.retryAttempts < 5;
    if (!state.disposed && canRetry) {
      state.retryAttempts += 1;
      state.retryTimer = window.setTimeout(() => {
        state.retryTimer = null;
        void syncWindow(state);
      }, 250 * state.retryAttempts);
    }
  } finally {
    state.syncing = false;
    if (state.needsSync && !state.disposed) {
      state.needsSync = false;
      window.setTimeout(() => void syncWindow(state), 0);
    }
  }
}

export function useRightPanelWindowCompensation(
  mode: RightPanelMode | null,
  open: boolean,
) {
  const stateRef = useRef<CompensationState>({
    mode,
    open,
    baseWidth: null,
    appliedDelta: 0,
    syncing: false,
    needsSync: false,
    retryAttempts: 0,
    retryTimer: null,
    disposed: false,
  });

  useEffect(() => {
    const state = stateRef.current;
    state.disposed = false;
    state.mode = mode;
    state.open = open;
    void syncWindow(state);

    let layoutTimer: number | undefined;
    let mutationTimer: number | undefined;
    let mutationObserver: MutationObserver | null = null;
    const resizeObserver = open && typeof ResizeObserver !== 'undefined'
      ? new ResizeObserver(() => {
          window.clearTimeout(layoutTimer);
          layoutTimer = window.setTimeout(() => void syncWindow(state), 80);
        })
      : null;
    const observeLayout = () => {
      if (!resizeObserver) return;
      resizeObserver.disconnect();
      document.querySelectorAll<HTMLElement>(
        '.workbench-body, .workbench-workspace, .workbench-groupchat-shell',
      ).forEach((element) => resizeObserver.observe(element));
      mutationObserver?.disconnect();
      const workspace = document.querySelector<HTMLElement>('.workbench-workspace');
      if (workspace) {
        mutationObserver?.observe(workspace, { childList: true, subtree: true });
      }
    };
    mutationObserver = open && typeof MutationObserver !== 'undefined'
      ? new MutationObserver(() => {
          window.clearTimeout(mutationTimer);
          mutationTimer = window.setTimeout(() => {
            observeLayout();
            void syncWindow(state);
          }, 80);
        })
      : null;
    observeLayout();

    // Lazy pages and responsive breakpoints can settle several paints after the
    // mode change. Re-measure briefly instead of relying on the first frame.
    const settleTimers = [180, 700, 1500, 3000].map((delay) => (
      window.setTimeout(() => {
        observeLayout();
        void syncWindow(state);
      }, delay)
    ));
    return () => {
      resizeObserver?.disconnect();
      mutationObserver?.disconnect();
      window.clearTimeout(layoutTimer);
      window.clearTimeout(mutationTimer);
      if (state.retryTimer !== null) {
        window.clearTimeout(state.retryTimer);
        state.retryTimer = null;
      }
      settleTimers.forEach((timer) => window.clearTimeout(timer));
    };
  }, [mode, open]);

  useEffect(() => {
    const state = stateRef.current;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    let resizeTimer: number | undefined;

    void (async () => {
      try {
        const win = getCurrentWindow();
        unlisten = await win.onResized(() => {
          if (disposed) return;
          window.clearTimeout(resizeTimer);
          resizeTimer = window.setTimeout(() => void syncWindow(state), 80);
        });
      } catch {
        /* Non-Tauri preview. */
      }
    })();

    return () => {
      disposed = true;
      window.clearTimeout(resizeTimer);
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    const state = stateRef.current;
    state.disposed = false;
    return () => {
      state.disposed = true;
    };
  }, []);
}
