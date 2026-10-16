use std::mem::size_of;

use ::kithara::assets::{
    AcquisitionResult, AssetResource, AssetSource, AssetStore, StorageBackend, WriteSide,
};
use kithara_integration_tests::{
    artifact_timeline::ArtifactTimeline,
    audio_artifact::{AssetReader, AudioArtifactSet, ReadSide, audio_artifact_path},
    bufpool_ext::{TestPools, pools},
};
use kithara_test_utils::kithara;
use tempfile::tempdir;

fn read(reader: &AssetReader<TestPools>) -> Vec<u8> {
    let len = reader.len().expect("committed artifact length");
    let mut bytes = vec![0_u8; usize::try_from(len).expect("artifact length fits usize")];
    let read = reader.read_at(0, &mut bytes).expect("read audio artifact");
    assert_eq!(read, bytes.len());
    bytes
}

#[kithara::test(native, flash(false))]
fn timeline_svg_preserves_output_and_source_coordinates() {
    let temp = tempdir().expect("temporary artifact directory");
    let set =
        AudioArtifactSet::new(temp.path(), "timeline", 48_000, 2).expect("audio artifact set");
    let mut timeline = ArtifactTimeline::default();
    timeline.span(
        "deck<1",
        100,
        200,
        "presented",
        "beat & span",
        Some((1_000, 1_120)),
    );

    let reader = set
        .write_bytes("timeline.svg", timeline.svg().as_bytes())
        .expect("write timeline SVG");
    let svg = String::from_utf8(read(&reader)).expect("timeline is UTF-8");

    assert!(svg.contains("deck&lt;1"));
    assert!(svg.contains("beat &amp; span: output 100..200, source 1000..1120"));
    assert!(svg.contains("class=\"event presented\""));
}

#[kithara::test(native, flash(false))]
fn timeline_coalesces_adjacent_presented_spans() {
    let mut timeline = ArtifactTimeline::default();
    timeline.span("deck", 0, 128, "presented", "PCM", Some((20, 148)));
    timeline.span("deck", 128, 256, "presented", "PCM", Some((148, 276)));
    timeline.record_probes(&[]);

    assert_eq!(timeline.events().len(), 1);
    assert!(timeline.svg().contains("output 0..256, source 20..276"));
}

/// A CI runner keeps the artifact root between jobs and its containers reuse
/// process ids, so an earlier job leaves committed captures under the ids the
/// next job's process starts with.
#[kithara::test(native)]
fn an_earlier_run_with_the_same_process_id_does_not_block_a_new_capture() {
    let temp = tempdir().expect("temporary artifact directory");
    let store = AssetStore::builder(pools())
        .backend(StorageBackend::Disk {
            root: temp.path().to_path_buf(),
        })
        .build();
    for attempt in 0..16 {
        let source = AssetSource::Local {
            path: temp
                .path()
                .join(format!("reused-{}-{attempt}", std::process::id())),
        };
        let scope = store
            .scope::<AudioArtifactSet>(&source)
            .expect("earlier run scope");
        let key = scope
            .key(&AssetResource::Named {
                namespace: "artifacts".to_owned(),
                name: "capture.wav".to_owned(),
            })
            .expect("earlier run key");
        let AcquisitionResult::Pending(writer) = store
            .acquire_resource(&key, None)
            .expect("earlier run acquisition")
        else {
            panic!("earlier run found its capture already committed");
        };
        writer.write_at(0, b"earlier").expect("earlier run write");
        writer.commit(Some(7)).expect("earlier run commit");
    }

    let set = AudioArtifactSet::new(temp.path(), "reused", 48_000, 1).expect("audio artifact set");
    let mut recording = set
        .recording("capture", Some(1))
        .expect("new run recording");
    recording.push(&[0.5]).expect("record samples");
    let bytes = read(&AudioArtifactSet::finish(recording).expect("finish recording"));

    assert_eq!(&bytes[0..4], b"RIFF");
}

#[kithara::test(native, flash(false))]
fn recording_core_writes_float_wav_through_disk_assets() {
    let temp = tempdir().expect("temporary artifact directory");
    let set = AudioArtifactSet::new(temp.path(), "header", 48_000, 2).expect("audio artifact set");
    let mut recording = set.recording("capture", Some(2)).expect("recording");
    recording
        .push(&[0.25, -0.25, 0.5, -0.5])
        .expect("record samples");
    let reader = AudioArtifactSet::finish(recording).expect("finish recording");
    let bytes = read(&reader);

    assert_eq!(&bytes[0..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");
    assert_eq!(u16::from_le_bytes([bytes[20], bytes[21]]), 3);
    assert_eq!(bytes.len(), 44 + 4 * size_of::<f32>());
    assert!(
        audio_artifact_path(&reader)
            .expect("artifact path")
            .is_absolute()
    );
}

#[kithara::test(native, flash(false))]
fn recording_core_preserves_payload_across_packet_boundary() {
    let temp = tempdir().expect("temporary artifact directory");
    let set = AudioArtifactSet::new(temp.path(), "packets", 48_000, 1).expect("audio artifact set");
    let samples = (0..1_026)
        .map(|index| {
            f32::from_bits(0x3e80_0000 + u32::try_from(index).expect("sample index fits u32"))
        })
        .collect::<Vec<_>>();
    let mut recording = set.recording("batched", Some(1_026)).expect("recording");
    recording.push(&samples).expect("record samples");
    let bytes = read(&AudioArtifactSet::finish(recording).expect("finish recording"));
    let expected = samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect::<Vec<_>>();

    assert_eq!(&bytes[44..], expected);
}
