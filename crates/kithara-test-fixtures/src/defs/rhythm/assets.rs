use kithara_analysis::BeatArtifact;
use kithara_test_macros as kithara;

use super::score::{self, Control, Style};

#[kithara::asset(ext = "wav", content_type = "audio/wav")]
#[case::ambient_dub_62_aligned(Style::AmbientDub, Control::Aligned)]
#[case::ambient_dub_62_one_frame_late(Style::AmbientDub, Control::OneFrameLate)]
#[case::ambient_dub_62_one_beat_bar_late(Style::AmbientDub, Control::OneBeatBarLate)]
#[case::ambient_dub_62_missing_beat(Style::AmbientDub, Control::MissingBeat)]
#[case::trip_hop_74_aligned(Style::TripHop, Control::Aligned)]
#[case::trip_hop_74_one_frame_late(Style::TripHop, Control::OneFrameLate)]
#[case::trip_hop_74_one_beat_bar_late(Style::TripHop, Control::OneBeatBarLate)]
#[case::trip_hop_74_missing_beat(Style::TripHop, Control::MissingBeat)]
#[case::downtempo_96_aligned(Style::Downtempo, Control::Aligned)]
#[case::downtempo_96_one_frame_late(Style::Downtempo, Control::OneFrameLate)]
#[case::downtempo_96_one_beat_bar_late(Style::Downtempo, Control::OneBeatBarLate)]
#[case::downtempo_96_missing_beat(Style::Downtempo, Control::MissingBeat)]
#[case::house_124_aligned(Style::House, Control::Aligned)]
#[case::house_124_one_frame_late(Style::House, Control::OneFrameLate)]
#[case::house_124_one_beat_bar_late(Style::House, Control::OneBeatBarLate)]
#[case::house_124_missing_beat(Style::House, Control::MissingBeat)]
#[case::techno_132_aligned(Style::Techno, Control::Aligned)]
#[case::techno_132_one_frame_late(Style::Techno, Control::OneFrameLate)]
#[case::techno_132_one_beat_bar_late(Style::Techno, Control::OneBeatBarLate)]
#[case::techno_132_missing_beat(Style::Techno, Control::MissingBeat)]
#[case::breakbeat_140_aligned(Style::Breakbeat, Control::Aligned)]
#[case::breakbeat_140_one_frame_late(Style::Breakbeat, Control::OneFrameLate)]
#[case::breakbeat_140_one_beat_bar_late(Style::Breakbeat, Control::OneBeatBarLate)]
#[case::breakbeat_140_missing_beat(Style::Breakbeat, Control::MissingBeat)]
fn rhythm_wav(style: Style, control: Control) -> Vec<u8> {
    score::wav(style, control)
}

#[kithara::asset(
    ext = "beat",
    content_type = "application/x-kithara-beat",
    depends_on = ["rhythm_wav_{case}"]
)]
#[case::ambient_dub_62_aligned(Style::AmbientDub, Control::Aligned)]
#[case::ambient_dub_62_one_frame_late(Style::AmbientDub, Control::OneFrameLate)]
#[case::ambient_dub_62_one_beat_bar_late(Style::AmbientDub, Control::OneBeatBarLate)]
#[case::ambient_dub_62_missing_beat(Style::AmbientDub, Control::MissingBeat)]
#[case::trip_hop_74_aligned(Style::TripHop, Control::Aligned)]
#[case::trip_hop_74_one_frame_late(Style::TripHop, Control::OneFrameLate)]
#[case::trip_hop_74_one_beat_bar_late(Style::TripHop, Control::OneBeatBarLate)]
#[case::trip_hop_74_missing_beat(Style::TripHop, Control::MissingBeat)]
#[case::downtempo_96_aligned(Style::Downtempo, Control::Aligned)]
#[case::downtempo_96_one_frame_late(Style::Downtempo, Control::OneFrameLate)]
#[case::downtempo_96_one_beat_bar_late(Style::Downtempo, Control::OneBeatBarLate)]
#[case::downtempo_96_missing_beat(Style::Downtempo, Control::MissingBeat)]
#[case::house_124_aligned(Style::House, Control::Aligned)]
#[case::house_124_one_frame_late(Style::House, Control::OneFrameLate)]
#[case::house_124_one_beat_bar_late(Style::House, Control::OneBeatBarLate)]
#[case::house_124_missing_beat(Style::House, Control::MissingBeat)]
#[case::techno_132_aligned(Style::Techno, Control::Aligned)]
#[case::techno_132_one_frame_late(Style::Techno, Control::OneFrameLate)]
#[case::techno_132_one_beat_bar_late(Style::Techno, Control::OneBeatBarLate)]
#[case::techno_132_missing_beat(Style::Techno, Control::MissingBeat)]
#[case::breakbeat_140_aligned(Style::Breakbeat, Control::Aligned)]
#[case::breakbeat_140_one_frame_late(Style::Breakbeat, Control::OneFrameLate)]
#[case::breakbeat_140_one_beat_bar_late(Style::Breakbeat, Control::OneBeatBarLate)]
#[case::breakbeat_140_missing_beat(Style::Breakbeat, Control::MissingBeat)]
fn rhythm_expected_beat(_inputs: &[&[u8]], style: Style, control: Control) -> Vec<u8> {
    let mut bytes = Vec::new();
    BeatArtifact::from(score::truth(style, control)).write_to(&mut bytes);
    bytes
}
