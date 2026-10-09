# Sparse populated-index final review

Exact COM source SHA256: cf82a47eb29745f2c8c8ed93f2fe06092c1d333109b4fcf9eb55159b1d0b13c7. The populated-slot array is allocated once at max_queues. Empty queues remain publicly registered, but their ordinal offsets are neither read nor maintained. The populated list is private and ordered by registered queue slot. First-point insertion validates membership/list bounds before mutation; rejected host new-ID insertion restores exact previous Copy slot metadata and used_queues, while preserving sticky failure.

Verification:31 storage tests passed; focused vendor Clippy passed with the same existing deprecated and drain_collect exceptions. All matched debug/optimized full-matrix and empty-queue checksums agree with baseline. All post-constructor sparse-final stages measured0 allocations/frees/bytes. Earlier negative source snapshots and receipts remain unchanged in ../final/, ../empty-queue-diagnostic/, ../extended/ and ../maintained/.

Exact constructor accounting: INPUT8192queues/points =8198 allocations and1048696 requested bytes; OUTPUT4096 =4102 allocations and524408 bytes. Together this adds98352bytes over the dense-index version:98304bytes (96KiB) for ordered slot arrays plus48bytes of arena metadata. Excludes outer runtime ComWrapper<ParameterChanges>, allocator overhead and RSS.

Complexity: O(log populated queues) slot membership lookup, plus O(log points in the target queue) rank search except empty/tail fast paths. Insertion moves the global point-index suffix and updates only later populated queue offsets; first-point activation also shifts the populated slot suffix. Empty registered queues add no per-point offset-update walk. Arbitrary valid point reads remain O(1); dirty fallback remains bounded and checked. Large point suffixes can still be slower than the baseline per-queue Vec. These observations establish storage/allocation bounds, not callback deadline guarantees.

Five repetitions per case; tables are reused-block optimized milliseconds (min / median / max). Small32/128/512 cases configure matching budget sizes to isolate scaling; production budgets remain8192/4096. Full cold/reused/debug/optimized rows and exact commands are in comparison-report.md, summary.csv, diagnostic-report.md, empty-summary.csv, run-comparisons.sh and environment.txt.

## Full matrix: 32 points

| Workload / operation | Baseline min / median / max ms | Sparse min / median / max ms | Median ratio |
|---|---:|---:|---:|
| ascending / write | 0.000631 / 0.000731 / 0.001092 | 0.000711 / 0.000721 / 0.000812 | 0.986x |
| reverse / write | 0.000501 / 0.000520 / 0.000641 | 0.000931 / 0.001071 / 0.001182 | 2.060x |
| random / write | 0.000541 / 0.000590 / 0.000941 | 0.001042 / 0.001392 / 0.001953 | 2.359x |
| ascending / random_read | 0.000210 / 0.000220 / 0.000240 | 0.000280 / 0.000290 / 0.000300 | 1.318x |
| distinct_ids / write | 0.000501 / 0.000631 / 0.000691 | 0.001012 / 0.001142 / 0.001262 | 1.810x |
| alternating_write_read / write_read | 0.000771 / 0.000902 / 0.001022 | 0.001041 / 0.001051 / 0.001212 | 1.165x |
| multiqueue_last_interleave / write_read | 0.000892 / 0.000932 / 0.001101 | 0.001082 / 0.001111 / 0.001142 | 1.192x |
| multiqueue_nonlast_interleave / write_read | 0.000751 / 0.000852 / 0.000922 | 0.001412 / 0.001452 / 0.001472 | 1.704x |
| multiqueue_bulk / random_read | 0.000220 / 0.000261 / 0.000290 | 0.000301 / 0.000320 / 0.000371 | 1.226x |
| large_single_suffix / write_read | 0.000200 / 0.000210 / 0.000220 | 0.000330 / 0.000350 / 0.000430 | 1.667x |
| large_multiqueue_suffix / write_read | 0.000150 / 0.000181 / 0.000190 | 0.000521 / 0.000541 / 0.000561 | 2.989x |

## Full matrix: 128 points

| Workload / operation | Baseline min / median / max ms | Sparse min / median / max ms | Median ratio |
|---|---:|---:|---:|
| ascending / write | 0.004256 / 0.004327 / 0.004517 | 0.002614 / 0.002634 / 0.002674 | 0.609x |
| reverse / write | 0.002954 / 0.002975 / 0.003065 | 0.004276 / 0.004487 / 0.004497 | 1.508x |
| random / write | 0.004357 / 0.004647 / 0.004717 | 0.004146 / 0.004947 / 0.005699 | 1.065x |
| ascending / random_read | 0.000732 / 0.000751 / 0.000802 | 0.001031 / 0.001071 / 0.001072 | 1.426x |
| distinct_ids / write | 0.004157 / 0.004196 / 0.005779 | 0.007862 / 0.008032 / 0.008073 | 1.914x |
| alternating_write_read / write_read | 0.005538 / 0.005538 / 0.006730 | 0.003976 / 0.003986 / 0.004066 | 0.720x |
| multiqueue_last_interleave / write_read | 0.005408 / 0.005418 / 0.005518 | 0.004196 / 0.004316 / 0.004397 | 0.797x |
| multiqueue_nonlast_interleave / write_read | 0.005338 / 0.005388 / 0.005599 | 0.005719 / 0.005769 / 0.005789 | 1.071x |
| multiqueue_bulk / random_read | 0.000752 / 0.000782 / 0.000891 | 0.001092 / 0.001102 / 0.001172 | 1.409x |
| large_single_suffix / write_read | 0.000762 / 0.000821 / 0.000921 | 0.001512 / 0.001532 / 0.001542 | 1.866x |
| large_multiqueue_suffix / write_read | 0.000731 / 0.000771 / 0.000821 | 0.002323 / 0.002324 / 0.002354 | 3.014x |

## Full matrix: 512 points

| Workload / operation | Baseline min / median / max ms | Sparse min / median / max ms | Median ratio |
|---|---:|---:|---:|
| ascending / write | 0.044257 / 0.044438 / 0.077537 | 0.010295 / 0.010296 / 0.010366 | 0.232x |
| reverse / write | 0.024397 / 0.024687 / 0.025388 | 0.025288 / 0.025569 / 0.025689 | 1.036x |
| random / write | 0.038838 / 0.039009 / 0.039560 | 0.024647 / 0.029645 / 0.038178 | 0.760x |
| ascending / random_read | 0.002854 / 0.002935 / 0.002955 | 0.004056 / 0.004086 / 0.004106 | 1.392x |
| distinct_ids / write | 0.049425 / 0.049675 / 0.049805 | 0.085999 / 0.086050 / 0.099039 | 1.732x |
| alternating_write_read / write_read | 0.049535 / 0.049624 / 0.049695 | 0.015794 / 0.015804 / 0.231870 | 0.318x |
| multiqueue_last_interleave / write_read | 0.048823 / 0.048894 / 0.080061 | 0.016715 / 0.016735 / 0.018138 | 0.342x |
| multiqueue_nonlast_interleave / write_read | 0.048873 / 0.049044 / 0.079590 | 0.023235 / 0.023385 / 0.023616 | 0.477x |
| multiqueue_bulk / random_read | 0.002914 / 0.002935 / 0.003295 | 0.004176 / 0.004227 / 0.004266 | 1.440x |
| large_single_suffix / write_read | 0.005408 / 0.005568 / 0.005699 | 0.008513 / 0.008673 / 0.008713 | 1.558x |
| large_multiqueue_suffix / write_read | 0.005518 / 0.005569 / 0.099110 | 0.011888 / 0.011918 / 0.015013 | 2.140x |

## Full matrix: 4096 points

| Workload / operation | Baseline min / median / max ms | Sparse min / median / max ms | Median ratio |
|---|---:|---:|---:|
| ascending / write | 2.438242 / 2.554478 / 2.805236 | 0.082104 / 0.082374 / 0.098499 | 0.032x |
| reverse / write | 1.533396 / 1.668920 / 1.765246 | 0.753278 / 0.838125 / 2.360205 | 0.502x |
| random / write | 1.993010 / 2.041152 / 2.815222 | 0.595459 / 0.606316 / 0.742862 | 0.297x |
| ascending / random_read | 0.023255 / 0.023525 / 0.023695 | 0.032399 / 0.032609 / 0.093351 | 1.386x |
| distinct_ids / write | 5.348187 / 5.433356 / 5.774070 | 4.874201 / 5.436500 / 5.717224 | 1.001x |
| alternating_write_read / write_read | 2.443660 / 2.577714 / 2.763614 | 0.125821 / 0.126781 / 0.163457 | 0.049x |
| multiqueue_last_interleave / write_read | 2.527237 / 2.886880 / 5.545204 | 0.133942 / 0.134233 / 0.175145 | 0.046x |
| multiqueue_nonlast_interleave / write_read | 2.443040 / 2.501588 / 2.589401 | 0.185130 / 0.187423 / 0.533886 | 0.075x |
| multiqueue_bulk / random_read | 0.023475 / 0.023556 / 0.023756 | 0.034172 / 0.034853 / 0.035664 | 1.480x |
| large_single_suffix / write_read | 0.169396 / 0.170648 / 0.182756 | 0.255135 / 0.255576 / 0.316177 | 1.498x |
| large_multiqueue_suffix / write_read | 0.169706 / 0.169847 / 0.187544 | 0.285041 / 0.285952 / 0.319813 | 1.684x |

## Full matrix: 8192 points

| Workload / operation | Baseline min / median / max ms | Sparse min / median / max ms | Median ratio |
|---|---:|---:|---:|
| ascending / write | 9.503953 / 9.699229 / 9.986573 | 0.164508 / 0.179552 / 0.355968 | 0.019x |
| reverse / write | 6.194275 / 6.527188 / 7.143058 | 3.355528 / 3.540667 / 3.849484 | 0.542x |
| random / write | 7.813070 / 8.192433 / 9.108887 | 1.944857 / 2.005118 / 2.799108 | 0.245x |
| ascending / random_read | 0.047171 / 0.047271 / 0.096546 | 0.065779 / 0.066571 / 0.132440 | 1.408x |
| distinct_ids / write | 23.443646 / 24.543127 / 28.407202 | 19.871131 / 21.217835 / 21.618084 | 0.865x |
| alternating_write_read / write_read | 9.824317 / 10.060885 / 10.345224 | 0.251039 / 0.287875 / 0.332412 | 0.029x |
| multiqueue_last_interleave / write_read | 9.577725 / 10.030129 / 10.335459 | 0.267684 / 0.363720 / 0.742091 | 0.036x |
| multiqueue_nonlast_interleave / write_read | 9.687882 / 9.747031 / 9.906442 | 0.381446 / 0.455188 / 0.550010 | 0.047x |
| multiqueue_bulk / random_read | 0.046450 / 0.046660 / 0.048333 | 0.070166 / 0.071528 / 0.120853 | 1.533x |
| large_single_suffix / write_read | 0.621959 / 0.657043 / 0.892518 | 1.285632 / 1.293363 / 1.365262 | 1.968x |
| large_multiqueue_suffix / write_read | 0.650974 / 0.743633 / 0.865137 | 1.306553 / 1.677633 / 2.081064 | 2.256x |

## Maximum empty registered queues

| Total queues | Edits | Baseline min / median / max ms | Sparse min / median / max ms |
|---:|---:|---:|---:|
| 4096 | 32 | 0.000791 / 0.001081 / 0.001352 | 0.001252 / 0.001473 / 0.002073 |
| 4096 | 128 | 0.006110 / 0.006130 / 0.006340 | 0.003846 / 0.004186 / 0.004627 |
| 8192 | 32 | 0.001352 / 0.001412 / 0.001642 | 0.001383 / 0.001802 / 0.002394 |
| 8192 | 128 | 0.006330 / 0.006560 / 0.006640 | 0.003916 / 0.004216 / 0.004376 |
