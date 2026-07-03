export ARCH := "$(uname -m)"
export OS := "$(uname -s | tr '[:upper:]' '[:lower:]')"

build:
	cargo build --lib --release --locked

# uniffi 0.31 --library mode: read the type metadata embedded in the compiled
# cdylib instead of parsing the UDL directly. Honors uniffi.toml
# (package_name / cdylib_name).
gen-python: build
	cargo run --bin uniffi-bindgen generate --library target/release/libflorestad_ffi.so --language python --out-dir generated/python --no-format

gen-tar: gen-python
	cp target/release/libflorestad_ffi.so generated/python/libflorestad_ffi.so
	tar cf "{{ARCH}}-{{OS}}.tar" generated/python/floresta.py generated/python/libflorestad_ffi.so

gen-kotlin: build
	cargo run --bin uniffi-bindgen generate --library target/release/libflorestad_ffi.so --language kotlin --out-dir generated/kotlin/ --no-format
