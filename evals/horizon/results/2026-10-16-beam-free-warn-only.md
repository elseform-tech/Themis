# Long horizon: beam-free

Runs: 47; infrastructure errors (excluded): 3. Benchmark audit, not an automated check.

| Task | Pass | Compactions | Notes used | Notes kept at checkpoints | Open at end | Progress reported | Early detail recalled | Count recalled | Re-reads | Loop warnings | Old guard would stop (passed) | Calls | Input tokens | Failure causes |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| modules-no-notes | 0% | 8.0 | 0% | - | n/a | n/a | 25% | 0% | 89.5 | 11.8 | 3 (0) | 113.5 | 349814.8 | unfinished 3, turn_limit 1 |
| modules-notes-advice | 60% | 5.4 | 80% | 100% of 22 | 0% | n/a | 60% | 60% | 21.2 | 0.0 | 0 (0) | 68.8 | 459375.4 | wrong_content 1, wrong_result 1 |
| modules-notes-offered | 0% | 6.8 | 100% | 100% of 27 | 0% | n/a | 50% | 0% | 68.5 | 3.2 | 3 (0) | 105.5 | 351558.8 | wrong_result 2, unfinished 2 |
| modules-turns-no-notes | 0% | 4.2 | 0% | - | n/a | 60% | 100% | 0% | 27.2 | 1.2 | 1 (0) | 70.4 | 318355.0 | wrong_content 1, unfinished 3, wrong_result 1 |
| modules-turns-notes-advice | 20% | 4.0 | 80% | 100% of 15 | 0% | 60% | 60% | 40% | 13.2 | 0.0 | 0 (0) | 49.2 | 399978.0 | unfinished 2, wrong_content 1, wrong_result 1 |
| modules-turns-notes-offered | 60% | 8.2 | 100% | 100% of 41 | 0% | 100% | 100% | 60% | 52.4 | 3.2 | 5 (3) | 113.2 | 505887.6 | unfinished 1, wrong_content 1 |
| ports-no-notes | 100% | 7.5 | 0% | - | n/a | n/a | 100% | 100% | 85.8 | 12.5 | 3 (3) | 113.5 | 295539.0 | - |
| ports-notes-offered | 80% | 5.2 | 100% | 100% of 23 | 20% | n/a | 80% | 80% | 47.0 | 3.2 | 2 (2) | 74.2 | 228473.6 | wrong_result 1 |
| stories-no-notes | 80% | 1.2 | 0% | - | n/a | n/a | 80% | 80% | 9.2 | 0.0 | 0 (0) | 35.0 | 107322.6 | unfinished 1 |
| stories-notes-offered | 100% | 1.2 | 100% | 100% of 6 | 0% | n/a | 100% | 100% | 0.6 | 0.0 | 0 (0) | 19.4 | 109121.6 | - |

