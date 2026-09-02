# kithara-ui - Context

What the code cannot say for itself. An entry belongs here only when it is
neither expressed by the shape of the code nor pinned by a test: an upstream
defect, a version pin with a reason outside this crate, or a boundary that no
single file owns. Everything else is in the code, and the code is the contract.

## Boundaries

- `doc/` - serde document types and validation. `render/` - the iced host and
  skin resolution. `draw/` - the toolkit-neutral command list, its builder, the
  backend trait, and replay. `solve/` - the neutral layout vocabulary and
  distribution shared by both hosts. `text/` - shaping through Parley.
  `backends/` - one module per rasteriser.
- `render::document` owns the compiled-document walk; a host owns only its
  toolkit tree, measurement, placement, paint replay, and event delivery. A host
  never re-walks the compiled document and never retains a parallel layout tree.
- `solve` holds the single layout answer because Masonry cannot express
  per-child minimums through its native flex protocol. Each host supplies
  measurement and placement only.
- The skin belongs to the application, not to the host that draws it. A document
  is compiled against a skin, so a host turning to another skin builds its pages
  again rather than repainting them.
- `InternId` is valid only within the `CompiledUi` that produced it. Never
  persist one in application messages or state; host-facing paths stay owned
  `String`s.
- `DrawBuffers` belongs to the host through `UiConfig`, not to the document. A
  configuration built per compile gives each document an empty pool family and
  throws the filled one away with the document.

## Dependency Constraints

These are facts about our dependencies. They are the reason for code that would
otherwise look arbitrary, and none of them can be read off our own sources.

- **Vello is pinned at 0.6** because masonry 0.4 hands a widget a `Scene` from
  that release, and a `Scene` from another Vello version is an unrelated type.
  The direct Vello dependency keeps its `wgpu` feature off; the backend only
  encodes commands.
- **wgpu 26 and 27 coexist on purpose.** Masonry brings Vello's wgpu 26 and iced
  brings wgpu 27. Cargo keeps the majors distinct and `deny.toml` reports the
  duplicate as a warning.
- **iced 0.14 has no radial gradient at any layer.** `IcedBackend::CAPS` sets
  `radial_gradient: false`, so a list holding `Paint::Radial` reaches the Vello
  host only, and `style()` answers `None` rather than substituting a colour that
  appears nowhere in the document.
- **vello#1198 is open.** In Vello 0.6, blend layers opened beneath
  `Scene::push_clip_layer` - including those used by COLR/CPAL and bitmap colour
  glyphs - may compose incorrectly before the outer clip is popped. Vello 0.9
  does not resolve it, and the 0.6 pin is required independently. Every Vello
  clip whose nested tree contains `GlyphFace::System` text emits one
  `tracing::warn!` naming the issue. That warning is the whole mitigation:
  replacing the clip, narrowing text to outline-only faces, or routing colour
  text around the clip would each invent a local rendering contract.
- **Parley 0.6, Vello 0.6, and the iced outline path share one skrifa 0.37
  instance,** so positioned glyph data crosses between shaping and rendering.
  Direct skrifa use belongs only to the iced outline adapter, because Parley
  does not expose glyph outlines.
- **Fontique 0.6.0 finds no CJK fallback on macOS.**
  `fontique-0.6.0/src/backend/coretext.rs` asks CoreText for a fallback, gets
  `PingFang SC`, then looks that family up in its own scanned name map, which
  does not contain it; `fallback()` returns `None`. A Han or Japanese title
  therefore shapes to `.notdef` there. Measured on an Apple/CoreText host over
  519 families, Hebrew, Arabic, Korean, Thai, and Devanagari all resolve. Do not
  select a face ourselves when Fontique declines: that would hide the upstream
  defect behind a private path.
- **The embedded faces do not cover the same scripts.** Inter and JetBrains Mono
  carry Latin, Cyrillic, and Greek; Space Grotesk is Latin-only. Because the
  shipped skins spend the Display family on titles, `TextResources::new`
  registers Inter as the collection fallback for Cyrillic and Greek. Greek is
  registered against a measurement, not an assumption, and a test holds it.

## Known Gap

Each text-drawing widget owns one `TextContext`, so a document holds as many
Parley shaping scratch buffers as it has shaped widgets. Consolidating them
needs a document-scoped owner that does not exist yet. Font-derived draw
resources already escaped that lifetime: `TextResources` builds the outline
collections and scans the system collection once, then lends them out.
