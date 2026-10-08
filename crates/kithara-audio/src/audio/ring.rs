#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    #[cfg(not(target_arch = "wasm32"))]
    #[kithara::test(hang_timeout_secs(1))]
    #[should_panic(expected = "recv_outcome_blocking")]
    fn blocking_recv_without_preload_panics_when_no_chunk_arrives() {
        let mut fixture = RingFixture::new(false);
        let _chunk = fixture.recv();
    }
}
