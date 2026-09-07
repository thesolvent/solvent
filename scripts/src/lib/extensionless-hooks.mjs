/**
 * The published `@1inch/*` ESM builds import their own files without a `.js` extension, which
 * Node's strict ESM resolver rejects. Retry any unresolved specifier with the extension appended.
 * (Vitest solves the same problem by inlining those packages.)
 */
export async function resolve(specifier, context, nextResolve) {
  try {
    return await nextResolve(specifier, context);
  } catch (error) {
    if (error?.code !== "ERR_MODULE_NOT_FOUND" || /\.[cm]?js$/.test(specifier)) throw error;
    return await nextResolve(`${specifier}.js`, context);
  }
}
