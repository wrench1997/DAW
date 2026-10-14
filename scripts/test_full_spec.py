from pathlib import Path
import os
import unittest
import build_full_spec as spec


def score(notes):
    return {"patterns": [{"id": 1, "notes": notes}], "clips": [{"id": 1, "pattern_id": 1,
        "kind": "Pattern", "muted": False, "start": 0, "length": 16, "source_offset": 0, "gain": 1}]}


def note(identifier, start, length, pitch=60, velocity=.5):
    return {"id": identifier, "channel_id": 1, "muted": False, "note": pitch,
            "start": start, "length": length, "velocity": velocity}


class FullSpecTests(unittest.TestCase):
    def test_complete_original_preserves_direct_master_and_all_boundaries(self):
        supplied = os.environ.get("CITRUS_NOIRE_FULL_QA_PROJECT")
        project = (Path(supplied).resolve(strict=True) if supplied else
                   Path(__file__).resolve().parents[1] / "target/melody-stability-20261011/desktop-song-13/After-Rain-NOIRE-Pure-native13.citrus")
        if not project.is_file():
            self.skipTest("the optional local Saved13 qualification input is not distributed in Git")
        built = spec.build(project.read_bytes(), 0)
        self.assertEqual((built["note_identities"], built["identity_boundaries"], built["wire_messages"]), (405, 810, 782))
        self.assertEqual(built["suppressed_same_pitch_closures"], 28)
        self.assertEqual(built["source_mixer_track_id"], 0xFFFFFFFF)
        self.assertEqual(built["geometry"]["blocks"], 43983)
        self.assertTrue(built["fixed_tail_may_be_silent"])

    def test_mutated_original_is_refused_before_projection(self):
        with self.assertRaisesRegex(ValueError, "immutable native13"):
            spec.build(b"{}", 0)

    def test_overlap_suppresses_only_off_while_other_identity_is_active(self):
        boundaries, wire = spec.expand_score(score([note(1, 0, 2), note(2, 1, 2)]))
        self.assertEqual([r["wire_emitted"] for r in boundaries], [True, True, False, True])
        self.assertEqual([r["message"][0] for r in wire], [0x90, 0x90, 0x80])

    def test_equal_frame_off_precedes_on_and_retriggers(self):
        boundaries, wire = spec.expand_score(score([note(1, 0, 1), note(2, 1, 1)]))
        self.assertEqual([r["kind"] for r in boundaries], ["on", "off", "on", "off"])
        self.assertEqual(len(wire), 4)
        self.assertEqual(wire[1]["frame"], wire[2]["frame"])

    def test_note_extent_clips_at_placement_and_velocity_zero_still_on_one(self):
        boundaries, wire = spec.expand_score(score([note(1, 15, 4, velocity=0)]))
        self.assertEqual(boundaries[1]["frame"], spec.sample_frame(16))
        self.assertEqual(wire[0]["message"], [0x90, 60, 1])

    def test_duplicate_identity_and_nonfinite_operands_refused(self):
        with self.assertRaisesRegex(ValueError, "duplicate placed"):
            spec.expand_score(score([note(1, 0, 1), note(1, 2, 1)]))
        invalid = score([note(1, float("nan"), 1)])
        with self.assertRaisesRegex(ValueError, "nonfinite"):
            spec.expand_score(invalid)

    def test_latency_changes_generated_ceiling_without_shortening_music(self):
        zero, changed = spec.geometry(0), spec.geometry(257)
        self.assertEqual(changed["musical_frames"], zero["musical_frames"])
        self.assertEqual(changed["retained_frames"], zero["retained_frames"])
        self.assertEqual(changed["generated_frames"] % 128, 0)
        self.assertGreaterEqual(changed["generated_frames"], 257 + zero["retained_frames"])
        for latency in (-1, True, 48000 * 30 + 1):
            with self.assertRaises(ValueError):
                spec.geometry(latency)


if __name__ == "__main__":
    unittest.main()
