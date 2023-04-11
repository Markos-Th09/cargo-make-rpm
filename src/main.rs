use std::{
    error::Error,
    fmt::{Display, Formatter},
    fs::{self, File},
    path::PathBuf,
    process::Command,
    str::FromStr,
};

use clap::{Parser, ValueEnum};
use regex::Regex;
use rpm::{signature::pgp::Signer, Dependency, RPMFileOptions};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
struct Manifest {
    packages: Vec<Package>,
    workspace_members: Option<Vec<String>>,
    workspace_root: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
struct Package {
    name: String,
    version: String,
    license: Option<String>,
    description: Option<String>,
    authors: Vec<String>,
    targets: Vec<Target>,
    manifest_path: String,
    metadata: Option<Metadata>,
    homepage: Option<String>,
    repository: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
struct Metadata {
    rpm: Option<RPMOptions>,
}

#[derive(Serialize, Deserialize, Debug)]
struct RPMOptions {
    #[serde(default)]
    compression: Compression,
    signing_key: Option<String>,
    dependencies: Option<Vec<String>>,
    conflicts: Option<Vec<String>>,
    assets: Option<Vec<(String, String, String)>>,
    preinstall: Option<String>,
    postinstall: Option<String>,
    preuninstall: Option<String>,
    postuninstall: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
struct Target {
    name: String,
    kind: Vec<String>,
}

#[derive(Parser)]
#[clap(version)]
struct Cli {
    #[clap(last = true, allow_hyphen_values = true, hide = true)]
    cargo_args: Vec<String>,
    /// Compression algorithm to use
    #[clap(long)]
    compression: Option<Compression>,
    /// Workspace member name to build
    #[clap(long, short)]
    package: Option<String>,
    /// Target triple to build for
    #[clap(long)]
    target: Option<String>,
    /// Signing key to use
    #[clap(long, short = 'k')]
    signing_key: Option<String>,
}

#[derive(ValueEnum, Default, Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Compression {
    None,
    #[default]
    Gzip,
    Zstd,
}

#[derive(Debug)]
struct Triplet {
    arch: String,
    vendor: String,
    os: String,
    libc: Option<String>,
}

impl Triplet {
    fn rpm_arch(&self) -> Option<String> {
        match self.arch.as_str() {
            "x86_64" => Some("x86_64".to_owned()),
            "aarch64" => Some("aarch64".to_owned()),
            "armv7" | "arm" if self.libc.as_ref().unwrap().ends_with("hf") => {
                Some("armhfp".to_owned())
            }
            "i386" => Some("i386".to_owned()),
            "powerpc64" => Some("ppc64".to_owned()),
            "powerpc64le" => Some("ppc64le".to_owned()),
            "s390x" => Some("s390x".to_owned()),
            _ => None,
        }
    }
}

impl Display for Triplet {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut triplet = format!("{}-{}-{}", self.arch, self.vendor, self.os);
        if let Some(libc) = &self.libc {
            triplet.push('-');
            triplet.push_str(libc);
        }

        write!(f, "{}", triplet)
    }
}

impl FromStr for Triplet {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.split('-');
        let arch = parts.next().ok_or("Invalid target triplet: No arch")?;
        let vendor = parts.next().ok_or("Invalid target triplet: No vendor")?;
        let os = parts.next().ok_or("Invalid target triplet: No os")?;
        let libc = parts.next();

        Ok(Triplet {
            arch: arch.to_owned(),
            vendor: vendor.to_owned(),
            os: os.to_owned(),
            libc: libc.map(|s| s.to_owned()),
        })
    }
}

fn pad_permission(mode: u16, filepath: &PathBuf) -> Result<u16, Box<dyn Error>> {
    let ftype = fs::metadata(filepath)?.file_type();
    if ftype.is_file() {
        Ok(0o100000 | mode)
    } else if ftype.is_dir() {
        Ok(0o040000 | mode)
    } else if ftype.is_symlink() {
        Ok(0o120000 | mode)
    } else {
        Err("invalid file type".into())
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Cli::parse();
    let metadata = Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn()?
        .wait_with_output()?
        .stdout;

    let manifest: Manifest = serde_json::from_slice(&metadata)?;
    let target = &args
        .target
        .as_ref()
        .cloned()
        .or_else(|| {
            let report = Command::new("rustc")
                .args(["--version", "--verbose"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn()
                .ok()?
                .wait_with_output()
                .ok()?
                .stdout;
            let report = String::from_utf8_lossy(&report);
            let regex = Regex::new(r"host: (.*)").unwrap();

            Some(regex.captures(&report)?.get(1)?.as_str().to_string())
        })
        .unwrap();
    let triplet = Triplet::from_str(target)?;

    if triplet.os != "linux" {
        eprintln!("warning: You are created for your current OS, not for Linux. Use --target to cross compile for a Linux target.");
    }

    let mut build = Command::new("cargo");
    build.args(["build", "--release"]);

    if let Some(ref target) = args.target {
        build.args(["--target", target]);
    }

    if let Some(ref package_name) = args.package {
        build.args(["-p", package_name]);
    }

    build.args(&args.cargo_args);
    build.spawn()?.wait()?;

    let packages = manifest
        .packages
        .into_iter()
        .filter(|p| args.package.as_ref().map_or(true, |n| &p.name == n));

    for package in packages {
        if !package
            .targets
            .iter()
            .any(|target| target.kind.contains(&"bin".to_owned()))
        {
            continue;
        }

        let crate_dir = manifest
            .workspace_root
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let crate_dir = PathBuf::from(&package.manifest_path);
                crate_dir.parent().unwrap().to_owned()
            });

        let base = crate_dir.join(PathBuf::from(format!(
            "target/{}/release",
            args.target.as_ref().cloned().unwrap_or(String::new())
        )));

        let rpm_path = base.join("../rpm");
        fs::create_dir_all(PathBuf::from(&rpm_path))?;

        let arch = triplet
            .rpm_arch()
            .ok_or(format!("Unsupported target arch: {}", triplet.arch))?;

        let options = package.metadata.as_ref().and_then(|m| m.rpm.as_ref());

        let mut rpm = rpm::RPMBuilder::new(
            &package.name,
            &package.version,
            package.license.as_ref().ok_or("Missing license")?,
            &arch,
            package
                .description
                .as_ref()
                .ok_or(format!("Missing description in crate {}", package.name))?,
        )
        .compression(rpm::Compressor::from_str(
            args.compression
                .unwrap_or(
                    package
                        .metadata
                        .as_ref()
                        .and_then(|m| m.rpm.as_ref().map(|r| r.compression))
                        .unwrap_or_default(),
                )
                .to_possible_value()
                .unwrap()
                .get_name(),
        )?);

        if !package.authors.is_empty() {
            rpm = rpm.vendor(package.authors.join(", "));
        }

        if let Some(ref homepage) = package.homepage {
            rpm = rpm.url(homepage);
        }

        if let Some(ref repository) = package.repository {
            rpm = rpm.vcs(format!("git:{repository}"));
        }

        for target in package.targets {
            if target.kind.contains(&"bin".to_owned()) {
                let path = base.join(&target.name);

                rpm = rpm.with_file(
                    path,
                    RPMFileOptions::new(format!("/usr/bin/{}", &target.name)).mode(0o100755),
                )?;
            }
        }

        if let Some(options) = options {
            if let Some(preinstall) = &options.preinstall {
                rpm = rpm.pre_install_script(preinstall);
            }

            if let Some(postinstall) = &options.postinstall {
                rpm = rpm.post_install_script(postinstall);
            }

            if let Some(preuninstall) = &options.preuninstall {
                rpm = rpm.pre_uninstall_script(preuninstall);
            }

            if let Some(postuninstall) = &options.postuninstall {
                rpm = rpm.post_uninstall_script(postuninstall);
            }

            if let Some(depedendecies) = &options.dependencies {
                for dep in depedendecies {
                    rpm = rpm.requires(Dependency::any(dep));
                }
            }

            if let Some(conflicts) = &options.conflicts {
                for conflict in conflicts {
                    rpm = rpm.conflicts(Dependency::any(conflict));
                }
            }

            if let Some(assets) = &options.assets {
                for (filename, asset, mode) in assets {
                    let filepath = PathBuf::from(filename).join(&crate_dir);
                    rpm = rpm.with_file(
                        &filepath,
                        RPMFileOptions::new(asset)
                            .mode(pad_permission(u16::from_str_radix(mode, 8)?, &filepath)?),
                    )?;
                }
            }
        }

        let signing_key = args
            .signing_key
            .as_ref()
            .or(options.and_then(|r| r.signing_key.as_ref()));

        let rpm_pkg = if let Some(signing_key) = signing_key {
            let signing_key = fs::read(PathBuf::from(signing_key).join(crate_dir))?;
            rpm.build_and_sign(Signer::load_from_asc_bytes(&signing_key)?)?
        } else {
            rpm.build()?
        };

        let mut rpm_file = File::create(rpm_path.join(PathBuf::from(format!(
            "{}-{}.{}.rpm",
            package.name, package.version, arch
        ))))?;

        rpm_pkg.write(&mut rpm_file)?;
    }

    Ok(())
}
