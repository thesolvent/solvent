/** Supported assets, as the symbols the filter cells offer. */
export interface AssetsPort {
  symbols(): Promise<string[]>;
}
