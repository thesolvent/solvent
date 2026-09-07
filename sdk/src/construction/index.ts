import { instructions } from "@1inch/swap-vm-sdk";

export { Strategy, bandToPrices } from "./strategy";
export type { TokenRef, PeggedTokenInfo, BuiltStrategy } from "./strategy";
export { coverage } from "./coverage";
export type { CoverageState, Coverage } from "./coverage";

// Re-expose the 1inch construction primitives as our own vocabulary, so consumers price and
// size positions through `@solvent/sdk/construction` rather than reaching into the 1inch SDK.
export const Price = instructions.concentrate.Price;
export const linearWidthFromSymmetricRangePercent =
  instructions.peggedSwap.linearWidthFromSymmetricRangePercent;
export const symmetricRangePercentFromLinearWidth =
  instructions.peggedSwap.symmetricRangePercentFromLinearWidth;
