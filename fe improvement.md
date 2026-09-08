# FE improvement

1. **RainbowKit UI and login/logout customization**
   Investigate how much we can customize RainbowKit's UI, including wallet connection, account display, and login/logout (connect/disconnect) interactions.

2. **Token approvals and allowance costs**
   Approvals are currently managed by the frontend. Investigate offering a one-time unlimited token allowance to Permit2 so subsequent swaps can reuse it and avoid repeated approval transaction fees. Read the on-chain allowance before requesting approval, explain the allowance choice clearly, and keep per-swap order signatures distinct from token approvals.

3. **Clearer swap widget explanations**
   Make the widget explain each step clearly: checking the quote and gas coverage, requesting token approval when needed, signing the order, submitting, and waiting for confirmation. Explain declines before wallet prompts when they can be detected, distinguish submitted from completed, and show the actual received amount and trade/explorer links after confirmation.

4. **Gas limits**
   Set min amounts for swaps and better handling blances and pricing
