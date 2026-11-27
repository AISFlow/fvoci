/** The wait a call of the backend in memory starts with: a turn of the loop at least, as an answer takes. */
export function pause(milliseconds: number): Promise<void> {
  if (milliseconds <= 0) return Promise.resolve();
  return new Promise((resolve) => {
    setTimeout(resolve, milliseconds);
  });
}
