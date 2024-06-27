version := "0.0.1"
revision := `git rev-parse --short HEAD`

run-example EXAMPLE:
  cargo run --example {{EXAMPLE}}

run-example-release EXAMPLE:
  cargo run --example {{EXAMPLE}} --release
