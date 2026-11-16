/** Build-time version injected from the root package.json by vite.config.ts. */
const injectedVersion = import.meta.env.VITE_APP_VERSION as string | undefined;

export const APP_VERSION = injectedVersion?.trim() || '0.0.0-dev';
