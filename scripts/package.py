#!/usr/bin/env python3
"""Build local install/remove products with Apple's pkgbuild and productbuild.

Only package construction happens here. Never installs, restarts audio, submits
to Apple, publishes a release or needs administrator rights.
"""
from pathlib import Path
import hashlib
import json
import os
import plistlib
import shutil
import subprocess
import tempfile
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parent.parent


def run(*args):
    subprocess.run([str(arg) for arg in args], check=True)


def write_script(path, text):
    path.write_text(text)
    path.chmod(0o755)


def distribution(action, version, component_name):
    identifier = 'net.batarelo.FastTrackUltra.' + action
    title = 'Fast Track Ultra' + (' Driver' if action == 'install' else ' Driver Removal')
    doc = ET.Element('installer-gui-script', {'minSpecVersion': '2'})
    ET.SubElement(doc, 'title').text = title + ' — Development Build'
    ET.SubElement(doc, 'options', {'customize': 'never', 'require-scripts': 'true', 'hostArchitectures': 'arm64'})
    ET.SubElement(doc, 'domains', {'enable_anywhere': 'false', 'enable_currentUserHome': 'false', 'enable_localSystem': 'true'})
    ET.SubElement(doc, 'welcome', {'file': action + '-welcome.html', 'mime-type': 'text/html'})
    ET.SubElement(doc, 'conclusion', {'file': action + '-conclusion.html', 'mime-type': 'text/html'})
    volume = ET.SubElement(doc, 'volume-check')
    versions = ET.SubElement(volume, 'allowed-os-versions')
    ET.SubElement(versions, 'os-version', {'min': '14.0'})
    outline = ET.SubElement(doc, 'choices-outline')
    ET.SubElement(outline, 'line', {'choice': 'driver'})
    choice = ET.SubElement(doc, 'choice', {'id': 'driver', 'visible': 'false', 'title': title})
    ET.SubElement(choice, 'pkg-ref', {'id': identifier})
    ET.SubElement(doc, 'pkg-ref', {'id': identifier, 'version': version, 'onConclusion': 'None'}).text = component_name
    return ET.tostring(doc, encoding='utf-8', xml_declaration=True)


def main():
    from build_identity import prepare
    prepare(ROOT)
    bundle = ROOT / 'build/FastTrackUltra.driver'
    run('/bin/sh', ROOT / 'scripts/deploy_hal.sh', 'check', bundle)
    info = plistlib.loads((bundle / 'Contents/Info.plist').read_bytes())
    metadata = json.loads((ROOT / 'build/build-identity.json').read_text())
    build_id = 'dev-' + metadata['identity'][:16]
    version = (ROOT / 'VERSION').read_text().strip()
    if info.get('FTUBuildIdentity') != build_id or info['CFBundleShortVersionString'] != version:
        raise SystemExit('Bundle and current build metadata disagree. Run make driver first.')
    out = ROOT / 'build/packages'
    out.mkdir(parents=True, exist_ok=True)
    manifest = {'version': version, 'build_id': build_id, 'source': metadata,
                'signing': 'Developer ID requested; not notarized' if os.getenv('INSTALLER_SIGN_IDENTITY') else 'unsigned development packages',
                'hardware_qualification': 'See docs/STATUS.md; this infrastructure build has not been installed or audio-tested.',
                'packaging_source_sha256': {
                    str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest()
                    for p in [ROOT / 'LICENSE', ROOT / 'scripts/package.py', ROOT / 'scripts/deploy_hal.sh',
                              *sorted((ROOT / 'packaging/resources').glob('*.html'))]},
                'files': {}}
    with tempfile.TemporaryDirectory(prefix='ftu-packaging-', dir=ROOT / 'build') as temporary:
        temp = Path(temporary)
        for action in ('install', 'remove'):
            work = temp / action
            scripts = work / 'scripts'
            scripts.mkdir(parents=True)
            shutil.copy2(ROOT / 'scripts/deploy_hal.sh', scripts / 'deploy_hal.sh')
            shutil.copy2(ROOT / 'LICENSE', scripts / 'LICENSE')
            if action == 'install':
                shutil.copytree(bundle, scripts / bundle.name)
                # Signing is also applied here so changing the explicit identity
                # does not depend on make deciding to relink a cached bundle.
                if os.getenv('SIGN_IDENTITY'):
                    run('/bin/sh', ROOT / 'scripts/sign.sh', scripts / bundle.name)
                run('/bin/sh', scripts / 'deploy_hal.sh', 'check', scripts / bundle.name)
                manifest['bundle_executable_sha256'] = hashlib.sha256(
                    (scripts / bundle.name / 'Contents/MacOS/FastTrackUltra').read_bytes()).hexdigest()
            write_script(scripts / 'preinstall', '''#!/bin/sh
set -eu
if [ "${3:-}" != / ]; then
    echo 'Fast Track Ultra can only be changed on the currently running system.' >&2
    exit 1
fi
if [ "$(/usr/bin/uname -m)" != arm64 ]; then
    echo 'Fast Track Ultra requires Apple Silicon.' >&2
    exit 1
fi
major=$(/usr/bin/sw_vers -productVersion | /usr/bin/cut -d . -f 1)
[ "$major" -ge 14 ] || exit 1
''')
            invocation = 'install "$here/FastTrackUltra.driver"' if action == 'install' else 'remove'
            write_script(scripts / 'postinstall', '''#!/bin/sh
set -eu
[ "${3:-}" = / ] || exit 1
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)
exec /bin/sh "$here/deploy_hal.sh" ''' + invocation + ' --restart\n')
            component = work / 'driver-component.pkg'
            run('/usr/bin/pkgbuild', '--nopayload', '--scripts', scripts,
                '--identifier', 'net.batarelo.FastTrackUltra.' + action,
                '--version', version, component)
            dist = work / 'Distribution.xml'
            dist.write_bytes(distribution(action, version, component.name))
            name = f'FastTrackUltra-{version}-{build_id}-' + ('install' if action == 'install' else 'uninstall') + '.pkg'
            product = work / name
            command = ['/usr/bin/productbuild', '--distribution', str(dist),
                       '--package-path', str(work), '--resources', str(ROOT / 'packaging/resources')]
            if os.getenv('INSTALLER_SIGN_IDENTITY'):
                command += ['--sign', os.environ['INSTALLER_SIGN_IDENTITY']]
            run(*command, product)
            # Parse the actual product without running its installation scripts.
            expanded = work / 'expanded'
            run('/usr/sbin/pkgutil', '--expand-full', product, expanded)
            ET.parse(expanded / 'Distribution')
            extracted = expanded / component.name / 'Scripts'
            if (extracted / 'deploy_hal.sh').read_bytes() != (ROOT / 'scripts/deploy_hal.sh').read_bytes():
                raise SystemExit('Packaged deployment script differs from the source')
            if (extracted / 'LICENSE').read_bytes() != (ROOT / 'LICENSE').read_bytes():
                raise SystemExit('Packaged MIT license differs from the source')
            if action == 'install':
                run('/bin/sh', extracted / 'deploy_hal.sh', 'check', extracted / bundle.name)
                if (extracted / bundle.name / 'Contents/Resources/LICENSE').read_bytes() != (ROOT / 'LICENSE').read_bytes():
                    raise SystemExit('Driver bundle does not contain the project MIT license')
            target = out / name
            shutil.copy2(product, target)
            manifest['files'][name] = hashlib.sha256(target.read_bytes()).hexdigest()
            print('Created:', target)
        manifest['deployment_script_sha256'] = hashlib.sha256((ROOT / 'scripts/deploy_hal.sh').read_bytes()).hexdigest()
        (out / f'FastTrackUltra-{version}-{build_id}-manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    print('Local development packages only. No system files or audio settings were changed.')


if __name__ == '__main__':
    main()
