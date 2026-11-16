import { useEffect, type FormEvent, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { Loader2, X } from 'lucide-react';

export interface WorkCreateProjectSummary {
  name: string;
  icon?: string;
  primaryDirectory?: string;
  linkedDirectoryCount?: number;
}

interface WorkCreateDialogShellProps {
  icon: ReactNode;
  kicker?: string;
  title: string;
  project?: WorkCreateProjectSummary | null;
  submitting?: boolean;
  onClose: () => void;
  onSubmit?: (event: FormEvent<HTMLFormElement>) => void;
  submitLabel: string;
  submittingLabel?: string;
  cancelLabel?: string;
  submitIcon?: ReactNode;
  footerLeading?: ReactNode;
  headerAction?: ReactNode;
  error?: string | null;
  children: ReactNode;
}

/**
 * Shared shell for creating sessions, rooms and workflows.
 * Keeping the frame and project context here prevents the three entry points
 * from drifting into subtly different dialogs again.
 */
export function WorkCreateDialogShell({
  icon,
  kicker = '项目工作',
  title,
  project,
  submitting = false,
  onClose,
  onSubmit,
  submitLabel,
  submittingLabel = '创建中…',
  cancelLabel = '取消',
  submitIcon,
  footerLeading,
  headerAction,
  error,
  children,
}: WorkCreateDialogShellProps) {
  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && !submitting) onClose();
    };
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [onClose, submitting]);

  return createPortal(
    <div className="projects-dialog-backdrop" role="presentation">
      <form
        className="projects-dialog"
        role="dialog"
        aria-modal="true"
        aria-label={title}
        onSubmit={onSubmit}
      >
        <div className="projects-dialog-head">
          <div className="min-w-0">
            <span className="projects-dialog-kicker">{kicker}</span>
            <h2>
              <span className="projects-work-create-title-icon">{icon}</span>
              {title}
            </h2>
          </div>
          <div className="projects-dialog-head-actions">
            {headerAction}
            <button
              type="button"
              className="projects-dialog-close"
              onClick={onClose}
              disabled={submitting}
              aria-label="关闭"
            >
              <X size={15} />
            </button>
          </div>
        </div>

        <div className="projects-dialog-body pd-scroll-stable">
          {project && (
            <div className="projects-work-create-project" aria-live="polite">
              <span className="projects-work-create-project-icon" aria-hidden="true">
                {project.icon || '📁'}
              </span>
              <div>
                <span>归属项目</span>
                <strong>{project.name}</strong>
              </div>
              {(project.primaryDirectory || project.linkedDirectoryCount) && (
                <small title={project.primaryDirectory}>
                  {project.primaryDirectory ? `主目录 · ${project.primaryDirectory}` : '跟随项目主目录'}
                  {project.linkedDirectoryCount
                    ? ` · ${project.linkedDirectoryCount} 个关联目录`
                    : ''}
                </small>
              )}
            </div>
          )}

          {children}

          {error && (
            <div className="projects-dialog-error" role="alert">
              {error}
            </div>
          )}
        </div>

        <div className="projects-dialog-footer">
          {footerLeading}
          <span className="flex-1" />
          <button
            type="button"
            className="projects-dialog-button is-secondary"
            onClick={onClose}
            disabled={submitting}
          >
            {cancelLabel}
          </button>
          <button
            type="submit"
            className="projects-dialog-button is-primary"
            disabled={submitting}
          >
            {submitting ? <Loader2 size={12} className="animate-spin" /> : submitIcon}
            {submitting ? submittingLabel : submitLabel}
          </button>
        </div>
      </form>
    </div>,
    document.body,
  );
}
