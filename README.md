# cargo-make-rpm
A tool to generate rpm packages from a rust project

## Usage
```
cargo make-rpm [--options] [--] [cargo options]
```

the packages will be written at the `target/rpm` or `target/[triplet]/rpm` directory dependending if the `--target` flag is used

## Arguments
```
    --compression <COMPRESSION>    Compression algorithm to use [possible values: none, gzip, zstd]
-p, --package <PACKAGE_NAME>  Workspace member name to build
    --target <TARGET>              Target triple to build for
-k, --signing-key <SIGNING_KEY>    Signing key to use
-h, --help                         Print help
-V, --version                      Print version
```

## Configuration
Some options can be configured in Cargo.toml using the `[package.metadata.rpm]` field

```toml
[package.metadata.rpm]
assets = [
    ["README.md", "/usr/share/doc/README.md", "644"]
]
compression = "none"
```

### Options
- compression: specify the compression (possible values: gzip, zstd, none)
- signing_key: path to the gpg private key
- dependencies: list of depedencies of the rpm
- conflicts: list of packages this package conflicts with
- assets: list of additional assets with the format [filepath, installation_path, permissions]
- preinstall: a command to run before installation
- postinstall: a command to run after installation
- preuninstall: a command to run before removal
- postinstall: a command to run after removal
