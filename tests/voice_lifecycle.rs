use shelloop::{VoiceAllocator, VoiceId};

#[test]
fn allocator_rejects_zero_or_unbounded_polyphony() {
    assert!(VoiceAllocator::new(0).is_err());
    assert!(VoiceAllocator::new(257).is_err());
    assert!(VoiceAllocator::new(8).is_ok());
}

#[test]
fn note_on_and_note_off_have_bounded_lifecycle() {
    let mut voices = VoiceAllocator::new(2).unwrap();
    let id = voices.note_on(0, 60, 0.75).unwrap();

    assert_eq!(voices.active_count(), 1);
    let state = voices.voice(id).unwrap();
    assert_eq!(state.note, 60);
    assert_eq!(state.channel, 0);
    assert!(state.key_held);

    assert_eq!(voices.note_off(0, 60), Some(id));
    assert_eq!(voices.active_count(), 0);
    assert!(!voices.is_active(id));
}

#[test]
fn repeated_note_off_releases_oldest_matching_voice_first() {
    let mut voices = VoiceAllocator::new(4).unwrap();
    let first = voices.note_on(1, 64, 1.0).unwrap();
    let second = voices.note_on(1, 64, 0.5).unwrap();

    assert_eq!(voices.note_off(1, 64), Some(first));
    assert!(!voices.is_active(first));
    assert!(voices.is_active(second));
    assert_eq!(voices.note_off(1, 64), Some(second));
}

#[test]
fn sustain_defers_release_until_pedal_is_released() {
    let mut voices = VoiceAllocator::new(4).unwrap();
    let id = voices.note_on(2, 67, 0.9).unwrap();
    voices.set_sustain(2, true);

    assert_eq!(voices.note_off(2, 67), None);
    let state = voices.voice(id).unwrap();
    assert!(state.active);
    assert!(!state.key_held);
    assert!(state.sustained);

    assert_eq!(voices.release_sustain(2), 1);
    assert!(!voices.is_active(id));
}

#[test]
fn voice_exhaustion_steals_oldest_releasable_then_oldest_active() {
    let mut voices = VoiceAllocator::new(2).unwrap();
    let first = voices.note_on(0, 60, 1.0).unwrap();
    let second = voices.note_on(0, 62, 1.0).unwrap();

    voices.set_sustain(0, true);
    assert_eq!(voices.note_off(0, 60), None);
    let replacement = voices.note_on(0, 64, 1.0).unwrap();

    assert!(!voices.is_active(first));
    assert!(voices.is_active(second));
    assert!(voices.is_active(replacement));
    assert_eq!(replacement.slot, first.slot);
    assert_ne!(replacement.generation, first.generation);

    // With no releasable voices left, the oldest actively held voice is stolen.
    let newest = voices.note_on(0, 65, 1.0).unwrap();
    assert!(!voices.is_active(second));
    assert!(voices.is_active(newest));
    assert_eq!(newest.slot, second.slot);
}

#[test]
fn stale_voice_ids_cannot_control_reused_slots() {
    let mut voices = VoiceAllocator::new(1).unwrap();
    let old = voices.note_on(0, 60, 1.0).unwrap();
    let current = voices.note_on(0, 61, 1.0).unwrap();

    assert!(!voices.is_active(old));
    assert!(voices.is_active(current));
    assert_eq!(old.slot, current.slot);
    assert_ne!(old.generation, current.generation);
    assert!(voices.voice(old).is_none());
}

#[test]
fn panic_invalidates_every_active_voice_and_clears_sustain() {
    let mut voices = VoiceAllocator::new(8).unwrap();
    let ids: Vec<VoiceId> = [60, 64, 67]
        .into_iter()
        .map(|note| voices.note_on(3, note, 0.8).unwrap())
        .collect();
    voices.set_sustain(3, true);

    assert_eq!(voices.panic(), 3);
    assert_eq!(voices.active_count(), 0);
    assert!(!voices.sustain(3));
    assert!(ids.into_iter().all(|id| !voices.is_active(id)));
}
