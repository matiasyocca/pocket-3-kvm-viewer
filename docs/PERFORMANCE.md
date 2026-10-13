# Performance measurements

## Method

Results in `performance/results.jsonl` are fixed-interval per-process/per-thread Linux `/proc` CPU samples, appended after every completed sample. CPU percentages use **one logical core**; divide by the recorded `logical_cores` for whole-machine capacity. The host has four logical CPUs. Each comparison uses a private D-Bus session, the same 960x540 window, and visible/unfocused, inactive-workspace, and returned states. No FPS or resolution reduction is used in the quality-preserving comparisons.

The original baseline is revision `e399aba`. The `*-confirm` variants have identical opt-in frame-counter instrumentation, so delivered FPS/caps can be checked in both. Instrumentation increments one relaxed atomic counter per buffer and emits a diagnostic every three seconds; normal production builds omit it. These are sink-delivery measurements, not measured monitor presentation FPS or end-to-end latency.

Run the sampler/harness tests:

```sh
python3 -B -m unittest discover -s scripts -p 'test_*.py' -v
```

Build a provisioned release binary and benchmark it (close other viewers first):

```sh
nix develop -c cargo build --release --locked --features benchmark,native-yuv
nix develop -c python3 -B scripts/benchmark.py ./target/release/pocket3-kvm-viewer --label my-variant --output docs/performance/results.jsonl --seconds 15 --repeats 3
```

The harness refuses an existing viewer, targets only a new viewer in its private process group, checks process starttimes and monitor-workspace visibility before/after each sample, bounds compositor calls, and terminates only its private process group. Changed visibility/identity causes an aborted sample, not an accepted record.

## Native YUV comparison (candidate 2)

Three 15-second samples per state; reverse-order confirmation followed the initial baseline/native screening. Medians:

| Variant | Visible CPU, one core | Hidden CPU, one core | Returned CPU, one core |
|---|---:|---:|---:|
| Original baseline, uninstrumented | 56.530% | 36.798% | 54.731% |
| Baseline confirmation, instrumented | 54.797% | 36.931% | 54.263% |
| Native YUV confirmation, instrumented | 21.598% | 1.467% | 21.532% |
| Actual Nix package, native YUV, no instrumentation | 22.399% | 1.467% | 22.465% |

The confirmation reduces visible CPU by 60.586% relative to its paired baseline. The sink receives the same 1920x1080/60 stream: baseline converts YUY2 to RGB; native YUV receives YUY2 directly. Excluding the initial acquisition interval, observed sink-delivery FPS was 59.665–60.332 (baseline) and 59.966–60.316 (native). No capture errors appeared in either confirmation log.

Scoped screenshots of both variants showed the same complete target console with legible green/white text and no obvious rendering artifacts; a changing target build line demonstrated live content. The screenshots remain private in scratch rather than committed. This does not establish full color-chart correctness, touchscreen forwarding, end-to-end latency, GPU energy use, or USB power savings.

RSS did not show a significant regression in these confirmation runs: baseline steady-state observations were 153900–159976 KiB; native 153260–157312 KiB. RSS is not total GPU/device allocation.

Status: accepted after independent review, exact packaged-build verification, three repeated samples per state, and unchanged source mode. The verified output was `/nix/store/kkz4yi9pj1kcli33jhvrazjy4mllwhdw-pocket3-kvm-viewer-0.1.0`. Plain Cargo compatibility remains available; the Nix package explicitly enables native-YUV against GTK >=4.20. `performance/frame-delivery.json` preserves the delivered-FPS/caps evidence. No claim of end-to-end zero-copy is made.

## Visibility-driven suspension (candidate 1)

The optional `background-throttle` feature uses GDK SUSPENDED/MINIMIZED and GTK map/unmap signals, not focus/input inactivity or polling. The Nix package enables it with native YUV. Three 15-second samples per state compare the native-YUV cleanup baseline against the combined build:

| Variant | Visible CPU, one core | Hidden CPU, one core | Returned CPU, one core |
|---|---:|---:|---:|
| Native YUV + cleanup | 21.065% | 1.533% | 21.132% |
| Plus visibility pause/resume | 21.466% | 0.000% | 21.465% |
| Exact Nix package, no instrumentation | 23.532% | 0.000% | 22.399% |

Hidden samples were 0.000%, 0.000%, and 0.067% of one core; zero is sampling resolution, not proof of zero work. The sink buffer counter stayed at 3064 throughout 15 complete paused diagnostic intervals, then resumed approximately 60 FPS (steady Playing median 59.990) at unchanged 1080p60 YUY2. There were no capture errors. Visible/unfocused operation remained live. Scoped before/returned screenshots showed the target console and an advancing build line, with no new visible corruption; compositor transparency was visible in both.

Ten additional real hide/return cycles retained the same pipeline and each returned to rendering. GStreamer trace timestamps measured state-request-to-first-sink-render median 26.600 ms, maximum 38.152 ms. This is not first newly captured HDMI content, compositor presentation, or end-to-end latency. Trace-enabled CPU samples are reliability evidence only and are excluded from the efficiency comparison. Synthetic lifecycle tests verify manual Stop intent, hidden start, graph/node retention, stale-error safety, ownership, and actual-Playing inhibition. These do not replace physical unplug/replug or touchscreen testing.

Paused capture retains the video node and last image; USB traffic, total energy and GPU allocations were not measured. Evidence and exact trace excerpts are in `performance/background-throttle.json`; production CPU samples are the `native-background` rows. Parent integration passed 20 regular tests plus nine selected pipeline fixtures and two visibility-observer fixtures with native-YUV/background/benchmark enabled. The exact Nix package `/nix/store/sbi1wpnxgqn2nwyvrvq9mwvjl9glqk88-pocket3-kvm-viewer-0.1.0` built and passed flake checks; its repeated hidden samples were all 0.000%. Its active sample medians were somewhat higher than the instrumented comparison, so no active-viewing improvement is attributed to suspension. Final independent safety re-review passed after the hardware-free diagnostic's NON_UNIQUE correction; visibility suspension is accepted without claiming an active-viewing CPU improvement.

## Transport probes (candidates 5 and 10)

`performance/transport.jsonl` contains bounded raw-source-to-fakesink probes. The first two auto-mode runs completed, then a third stalled. Later modes encountered UVC TRY_FMT EIO/USB probe timeouts; kernel logs also reported UVC DMA allocation failure and xHCI errors. The user subsequently reported successful capture in the viewer, and the native/baseline confirmations above completed normally.

`performance/transport-verdict.json` excludes the affected runs from efficiency comparisons. Their low CPU is a failed/no-frame path, not a performance improvement. No I/O-mode change is accepted from this dataset. Further transport testing must confirm live frames and avoid rapid reopen loops.

## Failed-pipeline cleanup (candidate 4)

Hardware-free synthetic source tests independently reproduced the baseline failure: one source file descriptor remained open and the pipeline remained Playing after an asynchronous error, or Paused after a failed Playing transition. The corrected lifecycle retires both to Null with zero retained source FDs, removes the watch outside its own callback, and leaves a replacement capture untouched by stale errors. Extra lifetime tests cover a Start-equivalent signal handler and a missing Start widget; watch/idle callbacks no longer retain that widget or capture state.

The integrated real-capture check at unchanged 1080p60 measured 21.065% visible, 1.533% hidden, and 21.132% returned CPU per core, with median sink delivery 59.994 FPS and no capture errors. These are consistent with the native-YUV baseline; no healthy-stream CPU saving is attributed to error cleanup. Its demonstrated net benefit is prompt release of failed-pipeline resources. Evidence is in `performance/resource-cleanup.json` and the `native-error-cleanup` raw samples. Final safety re-review passed; this resource-retirement fix is accepted without claiming a healthy-stream CPU improvement.

## GL and direct-import disposition (candidates 3 and 5)

A hardware-GL comparison probe delivered unchanged 1920x1080/60 source and sink buffers. Three 15-second visible samples measured native YUV 23.465% versus explicit GL upload/conversion/shared RGBA textures 7.733% of one core. GLMemory RGBA/2D, GLMemoryPBO allocator and Intel UHD (TGL GT2) Mesa OpenGL ES were verified; scoped images showed complete live console content. GL-only feature confirmation reproduced ~7.733%; its native fallback graph remained ~21.465%, not the ~96% all-features regression. These are CPU/sink-delivery results, not total power or panel latency.

GL was not promoted. The bounded production/fallback implementation finished synthetic tests and Clippy but did not supply a verified release binary, independent production review, real-device fallback or combined compositor verification. Its partial patch/manifest are preserved in `performance/deferred-gl/`; `performance/gl-screening.json` and `performance/probe-results.jsonl` preserve screening evidence. The current package does not enable GL or DMABuf backend features.

Direct DMABuf/DMA_DRM/YUYV import actually negotiated, delivered ~60 FPS and rendered target content. It nevertheless used 96.859% CPU with all probe features and 97.514% without GL support, versus ~23.465% native YUV. It was rejected, not mistaken for a capability failure or zero-copy efficiency win. See `performance/dmabuf-verdict.json`.

The reproducible comparison-tool source is archived as `performance/pipeline-probe-source.tar.gz`, separate from the production app. Screenshots stay private in scratch.

## Stopping point for the optimization campaign

Full candidate dispositions are in `performance/candidate-decisions.json` (all 15 ranks accounted for). Native YUV, failure-resource retirement and compositor suspension are retained in separate reviewed commits. The device supports no lower native resolution; an incidental logind CPU rewrite was not justified by 20 transition measurements (median 7.794 ms, max 11.112 ms). The opt-in ignored inhibitor benchmark in `src/inhibit.rs` reproduces that measurement without a display or capture.

Source 30 FPS screened at 3.733% GL CPU versus 7.733% at 60 FPS, with actual source/sink throughput ~30 versus ~60; this halves temporal resolution. No economy option was shipped and the default was not lowered. Other RGB, renderer, queue/thread, wider ownership and compiler experiments were deferred, not declared completed gains. Initial failed transport samples remain excluded. No further performance tests are planned as part of this campaign.

Physical suspended unplug/replug, touchscreen behavior, full color-chart correctness, total power and first fresh HDMI/panel latency remain unverified. Synthetic hotplug/manual-intent/lifetime gates and real return-to-render tests do not replace those checks. In particular, the broader pre-existing UI ownership audit was deferred; this report does not claim leak-free UI teardown.

## Remaining candidate decisions

Candidate 1 (visibility-driven suspension) is accepted after corrected final safety review and packaged verification. Candidate 3 (GL) is deferred: its bounded implementation did not finish release/production-review/hardware-fallback verification. All further optimization work is stopped at the user's wrap-up request. Candidate 4 (error cleanup) is accepted and committed. No unmeasured fix has been declared a net improvement. Quality-reducing source FPS/resolution changes will not silently replace the default stream; unsupported/ineffective candidates are to be recorded and dropped.

Total energy/power and full end-to-end latency have not been measured. CPU results alone do not prove a total-power reduction.
