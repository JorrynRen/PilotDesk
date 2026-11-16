/**
 * Format a filesystem path for display without changing the value used by the app.
 *
 * Windows canonicalization may return verbatim paths such as `\\?\E:\work`.
 * Those prefixes are useful to the OS but should not leak into the UI.
 */
export function normalizeDisplayPath(value?: string | null): string {
  const raw = value?.trim() ?? '';
  if (!raw || /^[a-z][a-z0-9+.-]*:\/\//i.test(raw)) return raw;

  let path = raw.replace(/\\/g, '/');

  if (/^\/\/\?\/UNC\//i.test(path)) {
    path = `//${path.slice(8)}`;
  } else if (path.startsWith('//?/')) {
    path = path.slice(4);
  } else if (path.startsWith('//./')) {
    path = path.slice(4);
  } else if (path.startsWith('/??/')) {
    path = path.slice(4);
  } else if (/^\/\/\?(?=[a-z]:)/i.test(path)) {
    path = path.slice(3);
  }

  const windowsPath = /^[a-z]:\//i.test(path) || path.startsWith('//') || raw.includes('\\');
  if (!windowsPath) {
    const leadingSlash = path.startsWith('/') ? '/' : '';
    const normalized = `${leadingSlash}${path.slice(leadingSlash.length).replace(/\/+/g, '/')}`;
    return normalized.length > 1 ? normalized.replace(/\/+$/, '') : normalized;
  }

  if (path.startsWith('//')) {
    const uncBody = path
      .replace(/^\/+/, '')
      .replace(/\//g, '\\')
      .replace(/\\+/g, '\\')
      .replace(/\\+$/, '');
    return uncBody ? `\\\\${uncBody}` : '\\\\';
  }

  const normalized = path.replace(/\//g, '\\').replace(/\\+/g, '\\');
  if (/^[a-z]:\\$/i.test(normalized)) return normalized;
  return normalized.replace(/\\+$/, '');
}

/**
 * Remove Windows verbatim prefixes embedded in tool output text.
 *
 * Unlike normalizeDisplayPath, this keeps the surrounding prose, JSON and code
 * untouched and only rewrites the technical `\\?\` prefix.
 */
export function stripVerbatimPathsInText(value?: string | null): string {
  return (value ?? '')
    .replace(/\\\\\?\\UNC\\/gi, '\\\\')
    .replace(/\\\\\?\\(?=[a-z]:\\)/gi, '');
}

/** Join path segments using the separator style of the root path. */
export function joinDisplayPath(root?: string | null, relativePath?: string | null): string {
  const normalizedRoot = normalizeDisplayPath(root);
  const relative = relativePath?.trim().replace(/^[\\/]+/, '') ?? '';
  if (!normalizedRoot) return normalizeDisplayPath(relative);
  if (!relative) return normalizedRoot;

  const separator = normalizedRoot.includes('\\') || /^[a-z]:/i.test(normalizedRoot)
    ? '\\'
    : '/';
  const tail = relative.replace(/[\\/]+/g, separator);
  return normalizeDisplayPath(`${normalizedRoot.replace(/[\\/]+$/, '')}${separator}${tail}`);
}

/** Return the final path segment for compact labels. */
export function pathBasename(value?: string | null): string {
  const normalized = normalizeDisplayPath(value);
  const withoutTrailingSeparator = normalized.replace(/[\\/]+$/, '');
  return withoutTrailingSeparator.split(/[\\/]/).pop() || normalized;
}

/** Normalize path spelling for equality checks; Windows paths are case-insensitive. */
export function normalizePathForComparison(value?: string | null): string {
  const normalized = normalizeDisplayPath(value).replace(/[\\/]+$/, '');
  return /^[a-z]:/i.test(normalized) || normalized.startsWith('\\\\')
    ? normalized.toLowerCase()
    : normalized;
}
