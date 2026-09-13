export function WalletConfigurationError() {
  return (
    <main className="startupError" role="alert">
      <h1>Wallet configuration required</h1>
      <p>Set VITE_PRIVY_APP_ID in fe/.env.local, then restart the frontend.</p>
    </main>
  );
}
