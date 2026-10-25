import difflib
import hashlib
import json
import os
import pathlib
import re
import shutil
import subprocess
import time


def instrument(source):
    assert hashlib.sha256(source.encode()).hexdigest() == 'c392ed8070ecc74794abf00677c48778101fbbfd375f029822961fa69ba06446'
    original = '''fn iced_runs_a_ramp_across_the_shape_it_fills() {
    assert!(
        ramp_runs_across(&through_iced(&ramped())),
        "the iced backend flattened the ramp"
    );
}'''
    replacement = '''fn iced_runs_a_ramp_across_the_shape_it_fills() {
    let rgba = through_iced(&ramped());
    let middle = consts::OUTER.y + consts::OUTER.h / 2.0;
    let near = sample(&rgba, consts::OUTER.x + 2.0, middle);
    let far = sample(&rgba, consts::OUTER.x + consts::OUTER.w - 2.0, middle);
    let directory = std::path::PathBuf::from(std::env::var_os("PR731_UI_OWNER_DIR").expect("diagnostic output directory"));
    std::fs::write(directory.join("gradient.rgba"), &rgba).expect("save diagnostic pixels");
    std::fs::write(directory.join("samples.txt"), format!("near={near} far={far} bytes={}\\n", rgba.len())).expect("save diagnostic samples");
    eprintln!("PR731 gradient: near={near}, far={far}, bytes={}", rgba.len());
    assert!(
        ramp_runs_across(&rgba),
        "the iced backend flattened the ramp: near={near}, far={far}"
    );
}'''
    assert source.count(original) == 1
    return source.replace(original, replacement)


def main():
    root = pathlib.Path.cwd()
    out = root / 'target' / 'parity' / 'pr731-ui-owner-results' / os.environ['GITHUB_RUN_ID']
    out.mkdir(parents=True, exist_ok=False)
    report = {'kind': 'UI diagnostic only', 'sourceHead': 'de4f418426edaea3f42e599dd27b532f7359da2d',
              'runId': os.environ['GITHUB_RUN_ID'], 'attempt': os.environ['GITHUB_RUN_ATTEMPT'],
              'benchmarkExecutions': 0, 'criterionTimingExecutions': 0, 'measuredBuilds': 0,
              'numericAcceptancePassed': False, 'rootCauseEstablished': False,
              'testPredicateChanged': False, 'productionEdits': 0,
              'policy': 'One original UI cohort with diagnostic pixels and logs. No retry, profile/feature/threshold change or host mutation. Diagnostic output can perturb scheduling; it cannot prove historical conditions.'}
    source = root / 'crates/kithara-ui/src/backends/conformance.rs'
    original = candidate = None
    commands = []

    def capture(args):
        commands.append(args)
        return subprocess.check_output(args, text=True).strip()

    code = 1
    try:
        report['controller'] = capture(['git', 'rev-parse', 'HEAD'])
        parents = [line[7:] for line in capture(['git', 'cat-file', '-p', 'HEAD']).splitlines() if line.startswith('parent ')]
        assert parents == [report['sourceHead']]
        report['controllerParents'] = parents
        assert not capture(['git', 'status', '--porcelain'])
        report['compiler'] = capture(['just', '--command', 'rustc', '-Vv'])
        report['cargo'] = capture(['just', '--command', 'cargo', '-Vv'])
        report['uname'] = capture(['uname', '-a'])
        report['sourceHashes'] = {name: hashlib.sha256((root / name).read_bytes()).hexdigest()
                                  for name in ['Cargo.toml', 'Cargo.lock', '.cargo/config.toml']}
        if shutil.which('nvidia-smi'):
            report['gpu'] = capture(['nvidia-smi', '--query-gpu=uuid,name,driver_version,memory.total',
                                     '--format=csv,noheader'])
        else:
            report['gpuUtilityPresent'] = False
        original = source.read_text()
        candidate = instrument(original)
        (out / 'original-conformance.rs').write_text(original)
        (out / 'diagnostic.patch').write_text(''.join(difflib.unified_diff(
            original.splitlines(True), candidate.splitlines(True),
            fromfile='a/crates/kithara-ui/src/backends/conformance.rs',
            tofile='b/crates/kithara-ui/src/backends/conformance.rs')))
        source.write_text(candidate)
        env = os.environ.copy()
        env['PR731_UI_OWNER_DIR'] = str(out)
        env['RUST_LOG'] = 'warn,wgpu_core::instance=debug,wgpu_hal=info'
        args = ['just', 'test', 'run', '--lane=ui', '--retries=0']
        commands.append(args)
        started = time.monotonic()
        with (out / 'ui.log').open('w') as log:
            process = subprocess.Popen(args, env=env, stdout=subprocess.PIPE,
                                       stderr=subprocess.STDOUT, text=True)
            for line in process.stdout:
                log.write(line)
                print(line, end='', flush=True)
            code = process.wait()
        report['exitCode'] = code
        report['elapsedSeconds'] = time.monotonic() - started
        report['diagnosticComplete'] = (out / 'gradient.rgba').exists() and (out / 'samples.txt').exists()
        if report['diagnosticComplete']:
            pixels = (out / 'gradient.rgba').read_bytes()
            assert len(pixels) == 96 * 96 * 4
            report['samples'] = (out / 'samples.txt').read_text().strip()
            report['pixelSha256'] = hashlib.sha256(pixels).hexdigest()
        clean = re.sub(r'\x1b\[[0-?]*[ -/]*[@-~]', '', (out / 'ui.log').read_text())
        report['testSummaries'] = [line for line in clean.splitlines() if 'Summary' in line]
    except Exception as error:
        report['error'] = repr(error)
        raise
    finally:
        if original is not None and candidate is not None:
            assert source.read_text() == candidate, 'concurrent source change'
            source.write_text(original)
            report['originalSourceRestored'] = source.read_text() == original
        (out / 'commands.json').write_text(json.dumps(commands, indent=2) + '\n')
        (out / 'diagnostic-result.json').write_text(json.dumps(report, indent=2) + '\n')
    return code


if __name__ == '__main__':
    raise SystemExit(main())
