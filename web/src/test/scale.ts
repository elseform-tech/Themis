/** Helpers for scale tests. These count work instead of timing it, so they give the same answer on every machine. */

/** A list that counts how many items are read from it. */
export function countedList<T>(items: T[]): { list: T[]; reads: () => number } {
  let reads = 0;
  const list = new Proxy(items, {
    get(target, key, receiver) {
      if (typeof key === "string" && /^\d+$/.test(key)) reads += 1;
      return Reflect.get(target, key, receiver) as unknown;
    },
  });
  return { list, reads: () => reads };
}

/** How the work grows with size, from the first and last size: 1 is linear, 2 is quadratic. */
export function growthRate(sizes: number[], work: (size: number) => number): number {
  const first = sizes[0]!;
  const last = sizes[sizes.length - 1]!;
  return Math.log(work(last) / work(first)) / Math.log(last / first);
}

export const SCALE_SIZES = [1000, 8000];
