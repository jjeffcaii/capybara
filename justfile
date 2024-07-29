version := "0.0.1"
revision := `git rev-parse --short HEAD`

alias r := run

run:
  cargo run --package capybara --bin capybara -- run -c testdata/capybara.yaml

run-example EXAMPLE:
  cargo run --example {{EXAMPLE}}

run-example-release EXAMPLE:
  cargo run --example {{EXAMPLE}} --release
