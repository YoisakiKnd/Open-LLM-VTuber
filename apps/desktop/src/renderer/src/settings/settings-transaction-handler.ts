export type SettingsTransactionHandler = () => void | Promise<void>;

export class RuntimeSettingsTransactionError extends Error {
  constructor(readonly cause: unknown) {
    super("Rust settings transaction failed");
    this.name = "RuntimeSettingsTransactionError";
  }
}

export async function runSettingsTransactionHandlers(
  handlers: SettingsTransactionHandler[],
): Promise<void> {
  for (const handler of handlers) await handler();
}
