let toastContainer: HTMLDivElement | null = null;

function getContainer(): HTMLDivElement {
  if (toastContainer && document.body.contains(toastContainer)) {
    return toastContainer;
  }
  toastContainer = document.createElement('div');
  toastContainer.id = 'pilotdesk-toast-container';
  document.body.appendChild(toastContainer);
  return toastContainer;
}

export function showToast(message: string, type: 'error' | 'success' | 'info' = 'error', duration = 3000) {
  const container = getContainer();
  const toast = document.createElement('div');
  toast.className = `pilotdesk-toast pilotdesk-toast-${type}`;
  toast.textContent = message;
  container.appendChild(toast);

  setTimeout(() => {
    if (document.body.contains(toast)) {
      toast.remove();
    }
  }, duration);
}
