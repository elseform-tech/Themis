# Scale tests

Scale tests catch work that grows faster than a chat does. They count work, such as list items read or markdown parses, instead of timing it, so they give the same answer on every machine and can fail a pull request. They use `web/src/test/scale.ts`, and they sit beside the code they cover as `*.scale.test.ts(x)`.

`growthRate` compares two sizes: 1 means the work grows with the chat, 2 means it grows with the square of the chat. `scale.test.ts` checks that it can tell those apart.

Write a scale test before fixing a slow path, and see it fail. Use the worst realistic input. Keep sizes at or below 8,000 messages, so a slow path fails in seconds and doesn't tie up the machine.

Timing measurements of growth are benchmark audits: they report a trend and don't fail a pull request.
