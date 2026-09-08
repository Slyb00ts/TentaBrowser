# rav1d packaging shim

The `rav1d` 1.0.0 and 1.1.0 tarballs on crates.io omit `src/arm/asm-offsets.h`,
which `src/arm/64/refmvs.S` and the film-grain assembly include, so the `asm`
feature fails on every arm64 target with:

```
src/arm/64/refmvs.S:28:10: fatal error: 'src/arm/asm-offsets.h' file not found
```

`asm-offsets.h` here is the unmodified file from the upstream tag `v1.1.0`
(BSD-2-Clause, VideoLAN and dav1d authors). `.cargo/config.toml` adds this
directory to `CPATH` for every compiler run inside the linktest workspace, so
the include resolves. Remove the shim once a rav1d release ships the header.
