export function shouldPreserveTouchedDraft(
  touched: boolean,
  draft: unknown,
  committed: unknown,
): boolean {
  return touched && JSON.stringify(draft) !== JSON.stringify(committed);
}
