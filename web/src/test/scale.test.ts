import { expect, it } from "vitest";
import { countedList, growthRate } from "./scale";

const numbers = (size: number) => Array.from({ length: size }, (_, index) => index);

it("tells linear work from quadratic work", () => {
  const linear = growthRate([100, 800], size => {
    const { list, reads } = countedList(numbers(size));
    list.filter(item => item >= 0);
    return reads();
  });
  const quadratic = growthRate([100, 800], size => {
    const { list, reads } = countedList(numbers(size));
    list.filter(item => !list.some(other => other === -item - 1));
    return reads();
  });
  expect(linear).toBeCloseTo(1, 1);
  expect(quadratic).toBeCloseTo(2, 1);
});
