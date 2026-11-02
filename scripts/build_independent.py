#!/usr/bin/env python3
"""Build the experimental hardware bundle; never installs or starts audio."""
import json, os, shutil, subprocess
from pathlib import Path
from build_identity import ROOT, prepare
PROFILE = "independent-4ms-twelve-slot"
def main():
    out = ROOT / "build" / PROFILE
    out.mkdir(parents=True, exist_ok=True)
    (out / "build-result.json").unlink(missing_ok=True)
    env = dict(os.environ, CARGO_TARGET_DIR=str(out / "target"), MACOSX_DEPLOYMENT_TARGET="14.0")
    def run(*args): subprocess.run(args, cwd=ROOT, env=env, check=True)
    run("cargo", "build", "--release", "--locked", "--offline", "-p", "ftu-stream", "--no-default-features", "--features", "independent-4ms")
    identity=prepare(ROOT,output_dir=out,profile=PROFILE)
    lib=out / "target/release/libftu_stream.a"
    native=["tools/ftu-probe/duplex.c", "tools/ftu-probe/completion_route.c"]
    flags=["-std=c11","-O2","-g","-Wall","-Wextra","-Werror","-Wno-deprecated-declarations","-DFTU_INDEPENDENT","-arch","arm64","-mmacosx-version-min=14.0"]
    frameworks=["-framework","CoreAudio","-framework","CoreFoundation","-framework","IOKit","-framework","Security","-liconv"]
    probe=out / "transport-profile"
    run("clang",*flags,"tests/transport-profile.c",*native,str(lib),*frameworks,"-o",str(probe))
    result=subprocess.check_output([str(probe),"4","12","26","1"],text=True)
    (out / "transport.json").write_text(result)
    bundle=out / "FastTrackUltra.driver"
    if bundle.exists(): shutil.rmtree(bundle)
    (bundle / "Contents/MacOS").mkdir(parents=True)
    (bundle / "Contents/Resources").mkdir()
    shutil.copy2(out / "Info.plist",bundle / "Contents/Info.plist")
    shutil.copy2(ROOT / "LICENSE",bundle / "Contents/Resources/LICENSE")
    run("clang",*flags,"-include",str(out / "build_identity.h"),"-bundle","driver/plugin.c",*native,str(lib),*frameworks,"-o",str(bundle / "Contents/MacOS/FastTrackUltra"))
    run("/bin/sh","scripts/sign.sh",str(bundle))
    (out / "build-result.json").write_text(json.dumps(dict(identity=identity,profile=PROFILE,geometry=json.loads(result),installed=False,hardware_tested=False),indent=2)+"\n")
    print(bundle)
if __name__ == "__main__": main()
