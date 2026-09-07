/**
 * The API seam. The SDK's client interface *is* the port: services depend on this type, and the
 * concrete fetch-backed instance is supplied by `adapters/http`. Tests substitute a fake.
 *
 * Imported from the `/client` subpath because the package barrel also pulls the construction and
 * positions modules, whose `@1inch` dependencies reach for Node built-ins the browser lacks.
 */
export type { SolventClient as SolventApi } from "@solvent/sdk/client";
