import Testing
@testable import Kithara

@Suite("KitharaPlayer identity state")
struct KitharaPlayerIdentityStateTests {
    @Test("stop rejects a current-item publication captured by an older observer")
    func stopRejectsStaleObserverPublication() {
        let state = KitharaPlayerIdentityState()
        let item = KitharaPlayerItem(
            url: "https://example.com/identity.mp3",
            audioId: 42,
            uuid: 123
        )
        state.register(item)
        let observerEmission = state.resolveCurrent(trackId: item.ffiTrackId)

        let stopEmission = state.clear()
        var observed: [Int64?] = []
        state.publish(stopEmission) { observed.append($0?.uuid) }
        state.publish(observerEmission) { observed.append($0?.uuid) }

        #expect(observed == [nil])
        #expect(state.currentItem() == nil)
        #expect(state.item(for: item.ffiTrackId) == nil)
    }
}
