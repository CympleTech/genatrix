# Spike 01 · macOS sandbox for the inference process

**Question** (design doc 10): can the macOS sandbox block all outbound network from a process while leaving a Unix socket listener, memory-mapped loading of multi-gigabyte weights, and GPU compute intact?

**Verdict: PASS.** All four criteria hold. The two-process design in doc 04 stands, and the phrase "the operating system guarantees the local model has no network" is accurate on this platform.

## Environment

- Mac mini, Apple M2 Pro, 16 GB, macOS 26.6.2 (25G83)
- `/usr/bin/sandbox-exec` with an SBPL profile
- Probe programs: a Python script for sockets and mmap, a Swift program for Metal compute. Spike code lives in the session scratchpad and is not part of the repository.

## Profile under test

```scheme
(version 1)
(allow default)
(deny network*)
(allow network-bind network-inbound network-outbound (subpath "<canonical run dir>"))
```

Everything is allowed except the network; the network is then re-opened for one directory only, where the process's own Unix socket lives.

## Results

| Check | Outside sandbox (control) | Inside sandbox |
|---|---|---|
| TCP connect to 1.1.1.1:443 | allowed | blocked, `EPERM` |
| UDP send to 1.1.1.1:53 | allowed | blocked, `EPERM` |
| DNS lookup (`getaddrinfo`) | allowed | blocked, `gaierror` |
| TCP listen on 127.0.0.1 | allowed | blocked, `EPERM` |
| Connect to another Unix socket (`/var/run/mDNSResponder`) | allowed | blocked, `EPERM` |
| Child process (`curl` via `/bin/sh`) reaching the network | allowed | blocked, exit 7 |
| `exec` of a different binary (`nc`) reaching the network | allowed | blocked |
| Bind and serve our own Unix socket; unsandboxed client round-trip | ok | ok, `pong:ping` |
| `mmap` a 4.07 GiB file, touch every 16 KiB page | 6.4 s cold, 0.7 s warm | 0.7 to 2.6 s (cache state), no penalty |
| Metal: create device, compile a compute shader from source at runtime, run 16.7 M-element kernel | Apple M2 Pro, 3 ms | Apple M2 Pro, 2 ms |

Restrictions are inherited by children and survive `exec`, so a compromised inference process cannot escape by spawning a helper.

## Two things learned

1. **Paths in the profile must be canonical.** `$TMPDIR` on macOS is `/var/folders/...`, a symlink to `/private/var/folders/...`. A `subpath` written with the symlinked form silently matches nothing and the bind fails with `EPERM`. Resolve with `realpath` before generating the profile. The first run of this spike failed only because of this.
2. **`AF_UNIX` paths are capped at 104 bytes** on macOS. The socket must live in a short directory. The design should place it under the Genatrix data directory or a dedicated short run directory, never under a deep path.

Five filter spellings were tried once the path was canonical, and all worked: plain `subpath`, `literal`, `(local unix-socket (subpath …))`, `(remote unix-socket (subpath …))`, and the split local/remote form. The plain `subpath` form is the simplest and is what the design will use.

## Caveats

- `sandbox-exec` is marked deprecated by Apple but is present and functional on the current OS and is used by Apple's own system profiles. Design doc 04 already records this as a risk to monitor; the fallback is a Network Extension content filter or, failing that, retracting the "OS-enforced" claim.
- The probe used Python and a small Swift program, not the real MLX inference binary. What was verified is the OS boundary: no network, Unix socket in, mmap, Metal device and runtime shader compilation. The MLX build itself is verified in the local-model spike once the Metal toolchain is installed.
- The profile grants `(allow default)` and only removes the network. Tightening file access to the model directory and the run directory is straightforward and should be done when the real `genatrix-infer` binary exists, since Python's needs muddy the picture.

## Impact on the design

- Doc 04: no change to the two-process split. Add the two learnings above as implementation notes: canonical paths, short socket path.
- Doc 08: the run directory for sockets must be short; `~/Library/Application Support/Genatrix/run/` is 47 characters plus the socket name, within the limit.
