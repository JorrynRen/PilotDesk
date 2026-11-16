import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import { errorMessage } from '../utils/errorMessage';
import type {
  ApiShareAccessInfo,
  ApiShareDistributionView,
  ApiShareMemberAccessView,
  ApiShareProviderView,
  ApiShareSourceView,
  ApiShareTokenSecret,
  ApiShareTokenView,
  ApiShareUsageQuery,
  ApiShareUsageView,
  CreateApiShareDistributionInput,
  CreateApiShareSourceInput,
  CreateApiShareTokenInput,
  CreatedApiShareDistributions,
  IssuedApiShareToken,
  OpenedApiShareConnection,
  UpdateApiShareSourceInput,
  UpdateApiShareTokenInput,
} from '../types/enclave';

let usageRequestSequence = 0;
let usageResultOrgId: string | null = null;

interface EnclaveApiShareState {
  sources: ApiShareSourceView[];
  providers: ApiShareProviderView[];
  tokens: ApiShareTokenView[];
  distributions: ApiShareDistributionView[];
  members: ApiShareMemberAccessView[];
  usage: ApiShareUsageView | null;
  access: ApiShareAccessInfo | null;
  loading: boolean;
  usageLoading: boolean;
  saving: boolean;
  error: string | null;
  reset: () => void;
  load: (orgId: string, canManage: boolean) => Promise<void>;
  loadUsage: (orgId: string, query: ApiShareUsageQuery) => Promise<void>;
  createSource: (orgId: string, input: CreateApiShareSourceInput) => Promise<boolean>;
  updateSource: (
    orgId: string,
    sourceId: string,
    input: UpdateApiShareSourceInput,
  ) => Promise<boolean>;
  deleteSource: (orgId: string, sourceId: string) => Promise<boolean>;
  blockMember: (orgId: string, userId: string, reason?: string) => Promise<boolean>;
  unblockMember: (orgId: string, userId: string) => Promise<boolean>;
  createToken: (
    orgId: string,
    input: CreateApiShareTokenInput,
  ) => Promise<IssuedApiShareToken | null>;
  updateToken: (
    orgId: string,
    tokenId: string,
    input: UpdateApiShareTokenInput,
  ) => Promise<boolean>;
  copyToken: (orgId: string, tokenId: string) => Promise<ApiShareTokenSecret | null>;
  revokeToken: (orgId: string, tokenId: string) => Promise<boolean>;
  archiveToken: (orgId: string, tokenId: string) => Promise<boolean>;
  deleteToken: (orgId: string, tokenId: string) => Promise<boolean>;
  createDistributions: (
    orgId: string,
    input: CreateApiShareDistributionInput,
  ) => Promise<CreatedApiShareDistributions | null>;
  markDistributionSent: (
    orgId: string,
    distributionId: string,
    messageId: string,
  ) => Promise<boolean>;
  revokeDistribution: (orgId: string, distributionId: string) => Promise<boolean>;
  archiveDistribution: (orgId: string, distributionId: string) => Promise<boolean>;
  deleteDistribution: (orgId: string, distributionId: string) => Promise<boolean>;
  openDistribution: (orgId: string, distributionId: string) => Promise<OpenedApiShareConnection | null>;
  markDistributionApplied: (orgId: string, distributionId: string) => Promise<boolean>;
}

export const useEnclaveApiShareStore = create<EnclaveApiShareState>((set, get) => ({
  sources: [],
  providers: [],
  tokens: [],
  distributions: [],
  members: [],
  usage: null,
  access: null,
  loading: false,
  usageLoading: false,
  saving: false,
  error: null,

  reset: () => {
    usageRequestSequence += 1;
    usageResultOrgId = null;
    set({
      sources: [],
      providers: [],
      tokens: [],
      distributions: [],
      members: [],
      usage: null,
      access: null,
      loading: false,
      usageLoading: false,
      saving: false,
      error: null,
    });
  },

  load: async (orgId, canManage) => {
    if (usageResultOrgId && usageResultOrgId !== orgId) {
      usageRequestSequence += 1;
      usageResultOrgId = null;
      set({ usage: null, usageLoading: false });
    }
    set({ loading: true, error: null });
    try {
      const [access, sources, tokens, distributions, members] = await Promise.all([
        invoke<ApiShareAccessInfo>('enclave_api_share_access_info', { orgId }),
        invoke<ApiShareSourceView[]>('enclave_api_share_sources_list', { orgId }),
        canManage
          ? invoke<ApiShareTokenView[]>('enclave_api_share_tokens_list', { orgId })
          : Promise.resolve([]),
        invoke<ApiShareDistributionView[]>('enclave_api_share_distributions_list', { orgId }),
        canManage
          ? invoke<ApiShareMemberAccessView[]>('enclave_api_share_members_list', { orgId })
          : Promise.resolve([]),
      ]);
      const providers = canManage
        ? await invoke<ApiShareProviderView[]>('enclave_api_share_provider_catalog', { orgId })
        : [];
      set({
        access,
        sources,
        tokens,
        distributions,
        members,
        providers,
        loading: false,
      });
    } catch (e) {
      set({ error: errorMessage(e), loading: false });
    }
  },

  loadUsage: async (orgId, query) => {
    const requestSequence = ++usageRequestSequence;
    const switchedOrg = usageResultOrgId !== null && usageResultOrgId !== orgId;
    usageResultOrgId = orgId;
    set({
      usageLoading: true,
      error: null,
      ...(switchedOrg ? { usage: null } : {}),
    });
    try {
      const usage = await invoke<ApiShareUsageView>('enclave_api_share_usage', {
        orgId,
        sourceId: query.sourceId ?? null,
        tokenId: query.tokenId ?? null,
        fromDay: query.fromDay ?? null,
        toDay: query.toDay ?? null,
      });
      if (requestSequence !== usageRequestSequence || usageResultOrgId !== orgId) return;
      set({ usage, usageLoading: false });
    } catch (e) {
      if (requestSequence !== usageRequestSequence || usageResultOrgId !== orgId) return;
      set({ error: errorMessage(e), usageLoading: false });
    }
  },

  createSource: async (orgId, input) => {
    set({ saving: true, error: null });
    try {
      const source = await invoke<ApiShareSourceView>('enclave_api_share_source_create', {
        orgId,
        input,
      });
      set((state) => ({ sources: [source, ...state.sources], saving: false }));
      return true;
    } catch (e) {
      set({ error: errorMessage(e), saving: false });
      return false;
    }
  },

  updateSource: async (orgId, sourceId, input) => {
    set({ saving: true, error: null });
    try {
      const source = await invoke<ApiShareSourceView>('enclave_api_share_source_update', {
        orgId,
        sourceId,
        input,
      });
      set((state) => ({
        sources: state.sources.map((item) => (item.id === source.id ? source : item)),
        saving: false,
      }));
      return true;
    } catch (e) {
      set({ error: errorMessage(e), saving: false });
      return false;
    }
  },

  deleteSource: async (orgId, sourceId) => {
    set({ saving: true, error: null });
    try {
      await invoke('enclave_api_share_source_delete', { orgId, sourceId });
      await get().load(orgId, true);
      set({ saving: false });
      return true;
    } catch (e) {
      set({ error: errorMessage(e), saving: false });
      return false;
    }
  },

  blockMember: async (orgId, userId, reason) => {
    set({ saving: true, error: null });
    try {
      const member = await invoke<ApiShareMemberAccessView>('enclave_api_share_member_block', {
        orgId,
        userId,
        reason: reason ?? null,
      });
      set((state) => ({
        members: state.members.map((item) => (item.userId === member.userId ? member : item)),
        saving: false,
      }));
      return true;
    } catch (e) {
      set({ error: errorMessage(e), saving: false });
      return false;
    }
  },

  unblockMember: async (orgId, userId) => {
    set({ saving: true, error: null });
    try {
      const member = await invoke<ApiShareMemberAccessView>('enclave_api_share_member_unblock', {
        orgId,
        userId,
      });
      set((state) => ({
        members: state.members.map((item) => (item.userId === member.userId ? member : item)),
        saving: false,
      }));
      return true;
    } catch (e) {
      set({ error: errorMessage(e), saving: false });
      return false;
    }
  },

  createToken: async (orgId, input) => {
    set({ saving: true, error: null });
    try {
      const issued = await invoke<IssuedApiShareToken>('enclave_api_share_token_create', {
        orgId,
        input,
      });
      set((state) => ({ tokens: [issued.view, ...state.tokens], saving: false }));
      return issued;
    } catch (e) {
      set({ error: errorMessage(e), saving: false });
      return null;
    }
  },

  updateToken: async (orgId, tokenId, input) => {
    set({ saving: true, error: null });
    try {
      const token = await invoke<ApiShareTokenView>('enclave_api_share_token_update', {
        orgId,
        tokenId,
        input,
      });
      set((state) => ({
        tokens: state.tokens.map((item) => (item.id === token.id ? token : item)),
        saving: false,
      }));
      return true;
    } catch (e) {
      set({ error: errorMessage(e), saving: false });
      return false;
    }
  },

  copyToken: async (orgId, tokenId) => {
    set({ error: null });
    try {
      const secret = await invoke<ApiShareTokenSecret>('enclave_api_share_token_secret', {
        orgId,
        tokenId,
      });
      set((state) => ({
        tokens: state.tokens.map((item) => (
          item.id === tokenId ? { ...item, copiedAt: item.copiedAt ?? Date.now() / 1000 } : item
        )),
      }));
      return secret;
    } catch (e) {
      set({ error: errorMessage(e) });
      return null;
    }
  },

  revokeToken: async (orgId, tokenId) => {
    set({ saving: true, error: null });
    try {
      await invoke('enclave_api_share_token_revoke', { orgId, tokenId });
      set((state) => ({
        tokens: state.tokens.map((item) => (
          item.id === tokenId
            ? { ...item, enabled: false, revokedAt: item.revokedAt ?? Math.floor(Date.now() / 1000) }
            : item
        )),
        saving: false,
      }));
      return true;
    } catch (e) {
      set({ error: errorMessage(e), saving: false });
      return false;
    }
  },

  archiveToken: async (orgId, tokenId) => {
    set({ saving: true, error: null });
    try {
      const token = await invoke<ApiShareTokenView>('enclave_api_share_token_archive', {
        orgId,
        tokenId,
      });
      set((state) => ({
        tokens: state.tokens.map((item) => (item.id === token.id ? token : item)),
        distributions: state.distributions.map((item) => (
          item.tokenId === token.id
            ? { ...item, archivedAt: item.archivedAt ?? token.archivedAt }
            : item
        )),
        saving: false,
      }));
      return true;
    } catch (e) {
      set({ error: errorMessage(e), saving: false });
      return false;
    }
  },

  deleteToken: async (orgId, tokenId) => {
    set({ saving: true, error: null });
    try {
      await invoke('enclave_api_share_token_delete', { orgId, tokenId });
      set((state) => ({
        tokens: state.tokens.filter((item) => item.id !== tokenId),
        distributions: state.distributions.filter((item) => item.tokenId !== tokenId),
        saving: false,
      }));
      return true;
    } catch (e) {
      set({ error: errorMessage(e), saving: false });
      return false;
    }
  },

  createDistributions: async (orgId, input) => {
    set({ saving: true, error: null });
    try {
      const created = await invoke<CreatedApiShareDistributions>(
        'enclave_api_share_distribution_create',
        { orgId, input },
      );
      set((state) => ({
        distributions: [...created.items, ...state.distributions],
        saving: false,
      }));
      return created;
    } catch (e) {
      set({ error: errorMessage(e), saving: false });
      return null;
    }
  },

  markDistributionSent: async (orgId, distributionId, messageId) => {
    try {
      const distribution = await invoke<ApiShareDistributionView>(
        'enclave_api_share_distribution_mark_sent',
        { orgId, distributionId, messageId },
      );
      set((state) => ({
        distributions: state.distributions.map((item) => (
          item.id === distribution.id ? distribution : item
        )),
      }));
      return true;
    } catch (e) {
      set({ error: errorMessage(e) });
      return false;
    }
  },

  revokeDistribution: async (orgId, distributionId) => {
    set({ saving: true, error: null });
    try {
      await invoke('enclave_api_share_distribution_revoke', { orgId, distributionId });
      set((state) => ({
        distributions: state.distributions.map((item) => (
          item.id === distributionId
            ? { ...item, revokedAt: Math.floor(Date.now() / 1000) }
            : item
        )),
        saving: false,
      }));
      return true;
    } catch (e) {
      set({ error: errorMessage(e), saving: false });
      return false;
    }
  },

  archiveDistribution: async (orgId, distributionId) => {
    set({ saving: true, error: null });
    try {
      const distribution = await invoke<ApiShareDistributionView>(
        'enclave_api_share_distribution_archive',
        { orgId, distributionId },
      );
      set((state) => ({
        distributions: state.distributions.map((item) => (
          item.id === distribution.id ? distribution : item
        )),
        saving: false,
      }));
      return true;
    } catch (e) {
      set({ error: errorMessage(e), saving: false });
      return false;
    }
  },

  deleteDistribution: async (orgId, distributionId) => {
    set({ saving: true, error: null });
    try {
      await invoke('enclave_api_share_distribution_delete', { orgId, distributionId });
      set((state) => ({
        distributions: state.distributions.filter((item) => item.id !== distributionId),
        saving: false,
      }));
      return true;
    } catch (e) {
      set({ error: errorMessage(e), saving: false });
      return false;
    }
  },

  openDistribution: async (orgId, distributionId) => {
    set({ error: null });
    try {
      return await invoke<OpenedApiShareConnection>(
        'enclave_api_share_distribution_open',
        { orgId, distributionId },
      );
    } catch (e) {
      set({ error: errorMessage(e) });
      return null;
    }
  },

  markDistributionApplied: async (orgId, distributionId) => {
    try {
      const distribution = await invoke<ApiShareDistributionView>(
        'enclave_api_share_distribution_mark_applied',
        { orgId, distributionId },
      );
      set((state) => ({
        distributions: state.distributions.map((item) => (
          item.id === distribution.id ? distribution : item
        )),
        error: null,
      }));
      return true;
    } catch (e) {
      set({ error: errorMessage(e) });
      return false;
    }
  },
}));
