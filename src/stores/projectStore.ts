import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import type { Project, ProjectInput, ProjectWorkItem } from '../types/project';
import { errorMessage } from '../utils/errorMessage';

interface ProjectState {
  projects: Project[];
  loading: boolean;
  error: string | null;
  loadProjects: () => Promise<void>;
  createProject: (input: ProjectInput) => Promise<Project>;
  updateProject: (projectId: string, input: ProjectInput) => Promise<Project>;
  archiveProject: (projectId: string) => Promise<Project>;
  restoreProject: (projectId: string) => Promise<Project>;
  loadWorkItems: (projectId: string) => Promise<ProjectWorkItem[]>;
}

function replaceProject(projects: Project[], next: Project): Project[] {
  return projects.map((project) => project.id === next.id ? next : project);
}

export const useProjectStore = create<ProjectState>((set) => ({
  projects: [],
  loading: false,
  error: null,

  loadProjects: async () => {
    set({ loading: true, error: null });
    try {
      const projects = await invoke<Project[]>('project_list', { includeArchived: true });
      set({ projects, loading: false });
    } catch (error) {
      set({ error: errorMessage(error), loading: false });
    }
  },

  createProject: async (input) => {
    try {
      const project = await invoke<Project>('project_create', { input });
      set((state) => ({
        projects: [project, ...state.projects],
        error: null,
      }));
      return project;
    } catch (error) {
      set({ error: errorMessage(error) });
      throw error;
    }
  },

  updateProject: async (projectId, input) => {
    try {
      const project = await invoke<Project>('project_update', { projectId, input });
      set((state) => ({
        projects: replaceProject(state.projects, project),
        error: null,
      }));
      return project;
    } catch (error) {
      set({ error: errorMessage(error) });
      throw error;
    }
  },

  archiveProject: async (projectId) => {
    try {
      const project = await invoke<Project>('project_archive', { projectId });
      set((state) => ({
        projects: replaceProject(state.projects, project),
        error: null,
      }));
      return project;
    } catch (error) {
      set({ error: errorMessage(error) });
      throw error;
    }
  },

  restoreProject: async (projectId) => {
    try {
      const project = await invoke<Project>('project_restore', { projectId });
      set((state) => ({
        projects: replaceProject(state.projects, project),
        error: null,
      }));
      return project;
    } catch (error) {
      set({ error: errorMessage(error) });
      throw error;
    }
  },

  loadWorkItems: async (projectId) => {
    return invoke<ProjectWorkItem[]>('project_work_items', { projectId });
  },
}));

