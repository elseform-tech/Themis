# Tool use: longcat-2.5-preview-free (this checkout)

Runs: 12; infrastructure errors (excluded): 0. Benchmark audit, not an automated check.

Task pass rate **100%** · first-try tool success **98%** · wasted calls 8% · calls per task 14.4 · input tokens per task 52867.8 · latency 135.4 s

| Tool | Calls | First-try success | Wasted | Reasons |
| --- | --- | --- | --- | --- |
| create_dir | 5 | 100% | 0% | - |
| list_dir | 13 | 100% | 0% | - |
| notes | 7 | 100% | 0% | - |
| read_file | 85 | 100% | 13% | other_error 11 |
| shell | 30 | 100% | 0% | - |
| write_file | 33 | 92% | 9% | bad_arguments 3 |

| Task | Tool | Pass | Calls | Repeated calls | Compactions | Notes used | Input tokens | First failure |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| long-job-no-notes | notes | 100% | 21.7 | 6.3 | 1.3 | 0% | 89305.3 | - |
| long-job-notes | notes | 100% | 20.0 | 0.7 | 1.0 | 33% | 69822.3 | - |
| notes-multi-step | notes | 100% | 8.7 | 0.0 | 0.0 | 100% | 21320.0 | - |
| notes-progress-turns | notes | 100% | 7.3 | 0.7 | 0.0 | 0% | 31023.7 | - |

