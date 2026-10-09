// macOS arm64 bundle from the executed native 30/30 termination run.
// Includes legacy hooks, ordinary-object classification and callback termination.
// Only this exact configuration is accepted; every other route fails closed.
const ARTIFACT_PINS: &[ArtifactPin] = &[ArtifactPin {
  target: "aarch64-apple-darwin",
  features: &["use_custom_libcxx"],
  native_debug: false,
  crt_static: false,
  archive_sha256: "f7393a5194ca5e8360b10c50b465944dad656fec071f7f82dddf01544aa6d8c3",
  binding_sha256: "d6594abe610e3364c13c0b78a901c2eaa3a6422afaebcafdee9c8af6950315b1",
  provenance_sha256: "29955d05d43e7c4d01181a9880ff6d99898c63851b6160302cfefa6e13bb8aa2",
}];
