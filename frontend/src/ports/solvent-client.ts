// The read/write API seam: the SDK's client interface is the port. Infrastructure supplies a
// concrete fetch-backed instance; application consumes this type via context.
export type { SolventClient } from "@solvent/sdk/client";
