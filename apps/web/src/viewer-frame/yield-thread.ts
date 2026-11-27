/** Lets the frame's other tasks (input, painting) run before long work goes on. */
export const yieldThread = (): Promise<void> =>
  new Promise((resolve) => {
    setTimeout(resolve, 0);
  });
