# Excalidraw Deep Capture — 2026-09-10

> Source: live https://excalidraw.com/ (version `2026-09-10T14:45:37Z-afa3a65`) via hyprfast CDP + stagehand. All data verified against real browser, not mocked.

## 1. Shortcuts (full HelpDialog, 80 entries)

**Tools:** `H` Hand (pan) · `V|1` Selection · `R|2` Rectangle · `D|3` Diamond · `O|4` Ellipse · `A|5` Arrow · `L|6` Line · `P|7` Draw (freedraw) · `T|8` Text · `N` Sticky note · `9` Insert image · `E|0` Eraser · `F` Frame · `K` Laser pointer · `B` Bucket fill · `I` / `Shift+S` / `Shift+G` Pick color from canvas · `Ctrl+Enter` Edit line/arrow points · `Enter` Edit text / add label · `Enter`/`Shift+Enter` New line (text editor) · `Esc`/`Ctrl+Enter` Finish editing · `A click×3` Curved arrow · `L click×3` Curved line · `double-click|Enter` Crop image · `Enter|Esc` Finish cropping · `Q` Keep selected tool active · `Ctrl` Prevent arrow binding · `Ctrl+K` Add/Update link · `Tab|Shift+Tab` Toggle shape type

**View:** `Ctrl++` Zoom in · `Ctrl+-` Zoom out · `Ctrl+0` Reset zoom · `Shift+1` Zoom to fit all · `Shift+2` Zoom to selection · `PgUp/PgDn` Page up/down · `Shift+PgUp/PgDn` Page left/right · `Alt+Z` Zen mode · `Alt+S` Snap to objects · `Ctrl+'` Toggle grid · `Alt+R` View mode · `Alt+Shift+D` Light/dark · `Alt+/` Canvas & Shape properties · `Ctrl+F` Find on canvas · `Ctrl+/` or `Ctrl+Shift+P` Command palette

**Editor:** `Ctrl+Arrow` Create flowchart from element · `Alt+Arrow` Navigate flowchart · `Space+drag|Wheel+drag` Move canvas · `Ctrl+Delete` Reset canvas · `Delete|Backspace` Delete · `Ctrl+X/C/V` Cut/Copy/Paste · `Ctrl+Shift+V` Paste plaintext · `Ctrl+A` Select all · `Shift+click` Add to selection · `Ctrl+click` Deep select · `Ctrl+drag` Deep box + prevent dragging · `Shift+Alt+C` Copy as PNG · `Ctrl+Alt+C/V` Copy/Paste styles · `Ctrl+Shift+[` Send to back · `Ctrl+Shift+]` Bring to front · `Ctrl+[` Send backward · `Ctrl+]` Bring forward · `Ctrl+Shift+Arrows` Align top/bottom/left/right · `Ctrl+D|Alt+drag` Duplicate · `Ctrl+Shift+L` Lock/unlock · `Ctrl+Z` Undo · `Ctrl+Shift+Z` Redo · `Ctrl+G` Group · `Ctrl+Shift+G` Ungroup · `Shift+H` Flip H · `Shift+V` Flip V · `S` Stroke color · `G` Background · `Shift+F` Font · `Ctrl+Shift+<|> ` Font size

## 2. Canvas Structure

- **DOM:** `.excalidraw-app` → `.excalidraw` (`--right-sidebar-width:302px`, `--ui-pointerEvents:all`) → `layer-ui__wrapper` (top: `FixedSideContainer` → `App-menu_top` left hamburger + `shapes-section` toolbar Island + top-right `plus-banner` + `Share` + `Library` (key `0`)) → footer left `Canvas actions` (zoom 100%, undo `button-undo`, redo `button-redo`) center encryption link right `help-icon` (`?`) → `excalidraw-textEditorContainer`, `excalidraw-contextMenuContainer`, `excalidraw-eye-dropper-container`, `SVGLayer>svg`, `excalidraw__canvas-wrapper` with **2 canvases**: `canvas.static` (render cache, `1882×858` physical `1448.46×660.769` logical) + `canvas.interactive` (input, same dims, `Drawing canvas` a11y name). Both re-render via `requestAnimationFrame` throttled by `window.EXCALIDRAW_THROTTLE_RENDER`.

- **React:** `#root` → `Excalidraw` component (lazy `mermaid-to-excalidraw` chunk `assets/mermaid-to-excalidraw-*.js`), props-driven. Live API found via `__reactFiber*` BFS at depth 21 → `memoizedProps.excalidrawAPI` with keys: `isDestroyed, updateScene, applyDeltas, mutateElement, updateLibrary, addFiles, resetScene, getSceneElements{Map}IncludingDeleted, history, setViewport, getViewportOffsets, getSceneElements, getAppState, getFiles, getName, registerAction, refresh, setToast, id, setActiveTool, setCursor, resetCursor, getEditorInterface...`

- **State:** `localStorage['excalidraw']` = `Array<ExcalidrawElement>` (array, not object; 1368 bytes in capture with 2 text elements). `localStorage['excalidraw-state']` = `AppState` JSON (40+ keys: `theme`, `currentItem*` (BackgroundColor `transparent`, StrokeColor `#1e1e1e`, FillStyle `solid`, FontFamily `5`, FontSize `20`, Opacity `100`, Roughness `1`, Roundness `round`, ArrowType `round`, StrokeWidthKey `medium`, TextAlign `left`), `activeTool{type:'selection',locked:false}`, `export*` (Background `true`, Scale `1`, EmbedScene `false`, DarkMode `false`), `gridSize 20/step 5`, `isBindingEnabled true`, `scrollX 1312 scrollY 1282 zoom{value:0.2}` in wide view, `viewBackgroundColor #ffffff`, `zenModeEnabled false`, etc.). `localStorage['excalidraw-collab']` + `version-files/version-dataState/i18nextLng/excalidraw-theme/__EXCALIDRAW_SHA__` + `window.EXCALIDRAW_ASSET_PATH` CDN `https://excalidraw.nyc3.cdn.digitaloceanspaces.com/oss/`.

## 3. Data Model (element JSON)

Base (every element): `id` (e.g. `hw_gsj1g`), `type`, `x,y,width,height`, `angle 0`, `strokeColor #1e1e1e`, `backgroundColor transparent`, `fillStyle solid|hachure|cross-hatch`, `strokeWidth 1|2|4`, `strokeStyle solid|dashed|dotted`, `roughness 0|1|2` (architect sketch), `opacity 10-100`, `groupIds[]`, `frameId null|id`, `index "a1"` (lexicographic z-order), `roundness {type:3}` or `null`, `seed 32bit`, `version`, `versionNonce 32bit`, `isDeleted bool`, `boundElements [{id,type}]`, `link null|url`, `locked bool`.

Type-specific:
- `text`: `text/originalText`, `fontSize 20-64`, `fontFamily 1-5 (Nunito/Excalifont/Comic…)`, `textAlign left|center|right`, `verticalAlign top|middle`, `containerId null|id`, `autoResize true`, `lineHeight 1.25`, `baseline`, `baseFontSize`.
- `arrow|line`: `points [[0,0],[dx,dy]]`, `lastCommittedPoint`, `startBinding/endBinding {elementId, focus, gap}`, `startArrowhead/endArrowhead null|arrow|bar|dot|triangle`, `elbowed bool`.
- `freedraw`: `points [(x,y)...]`, `pressures`, `simulatePressure true`, `lastCommittedPoint`.
- `image`: `fileId`, `status pending|saved`, `scale [1,1]`.
- `frame`: `name`, `children [id...]`, `isHovered`.
- `sticky_note` (skeleton `type:"stickynote"`): container with solid `#ffdf6b`, label auto-fit.
- `embeddable`: `validatedUrl`.
- `diamond|rectangle|ellipse` with optional `label {text, strokeColor, fontSize, textAlign}` → creates bound text child + `boundElements` linking.

## 4. Export & Function Map

**Main menu (hamburger, `main-menu-trigger`):** `Open Ctrl+O` (load `.excalidraw`/`.excalidrawlib` via `loadFromBlob`), `Save to…` (`serializeAsJSON` → `.excalidraw` download, Embed scene toggle, `window.EXCALIDRAW_EXPORT_SOURCE`), `Export image… Ctrl+Shift+E` → `ImageExportModal`: preview `filename` input (`Untitled-2026-09-04-1741`), toggles: Background (#fff on/off), Dark mode, Embed scene (base64 JSON inside PNG `tEXt`/`iCCP` & SVG `<metadata>` for round-trip), Scale `1×|2×|3×` radio, actions: **PNG** (`canvas.toBlob`/`toDataURL`), **SVG** (`exportToSvg` serializes to `<svg>`), **Copy to clipboard** (`navigator.clipboard.write` PNG). Also: `Live collaboration…` → room `#room=id` hash + `excalidraw-collab` encryption, `Command palette Ctrl+/`, `Find Ctrl+F`, `Help ?`, `Reset canvas`, `Preferences` submenu (appearance, language via `i18n`), `Excalidraw+/GitHub/X/Discord/Sign up`.

**Library:** sidebar `Library 0` → dockable panel `defaultSidebarDockedPreference`, items `LibraryItems[]` via `serializeLibraryAsJSON/mergeLibraryItems/useHandleLibrary`, `#addLibrary` URL token `parseLibraryTokensFromUrl`.

**Other functions:** `Copy to clipboard as PNG Shift+Alt+C`, Shapes toolbar `Q` lock, Shapes `More tools` (frame/laser/bucket/eye-dropper), Search, `Toggle grid`, `Zen mode`, `View mode`, `Snap Alt+S`, `View background`, `Font` picker, `Stroke/Background` pickers, `Align/Distribute/Group/Order/Flip/Duplicate/Lock/Link`, `Mermaid` import, `Embeddable` iframe, `Collab` cursor sharing.

## 5. All Functionalities Summarized

Drawing: rectangle/diamond/ellipse with hachure/cross-hatch/solid fill, arrow (sharp/round/elbow + arrowheads), line, freedraw (pressure), text (auto-resize), text-containers (rect/ellipse/diamond with bound label), labelled arrows (`label:{text}`), arrow bindings (`start:{type|id}`/`end:{id}` → snap to nearest shape edge with `isBindingEnabled`), sticky notes (auto-shrink font, `created` footer), images (drag-drop `9`, crop double-click), frames (`type:"frame", children:[ids], name`), laser pointer (`K`), bucket fill (`B`), eraser. Editing: selection bounding box + handles (resize/rotate), group `Ctrl+G`, z-order, align, flush, distribute, flip, lock, duplicate, copy/paste/styles. Canvas: hand `H` pan, grid, snap, zen, view mode, background color, scroll/zoom via `setViewport`/`sceneCoordsToViewportCoords`. Persistence: `serializeAsJSON`/`loadFromBlob`/`loadSceneOrLibraryFromBlob`, `getSceneVersion`, `getCommonBounds`, `elementsOverlappingBBox`, `isElementInsideBBox`. Collaboration, library, i18n, `useEditorInterface` (phone/tablet/desktop).

## 6. hyprfast Lightning Implementation

**Fast path:** `api_js_call` → `excalidrawAPI.getSceneElements/updateScene` via React Fiber BFS (~50-120ms per batch of 50, vs 2-4s pointer drag). Fallback `localStorage + reload` (900ms). Element synthesis fully in Rust (`base_element` + `apply_opts` + `expand_label` bound-text) matching skeleton spec `docs/@excalidraw/excalidraw/api/excalidraw-element-skeleton` → `convertToExcalidrawElements` semantics but executed natively.

**Tools (11 MCP + 11 CLI):** `excalidraw_open(url)`, `excalidraw_get_scene()`, `excalidraw_clear()`, `excalidraw_draw{type,x,y,width,height,x2,y2,text,label,strokeColor,backgroundColor,fillStyle,strokeWidth}`, `excalidraw_draw_batch{elements:[...]}` (one `updateScene`), `excalidraw_update_scene{elements,mode}`, `excalidraw_diagram{kind,params}` (see §7), `excalidraw_export{format,background,dark,embedScene,scale}`, `excalidraw_save{path}`, `excalidraw_view{scrollX,scrollY,zoom}`, `excalidraw_fit()` (bbox centering `zoom 0.7`).

**Diagrams (§7):** `flowchart{steps[],title}`, `sequence{participants[],messages[]}`, `microservices|architecture|aws|3tier{services[],databases[],title}` (API Gateway→services grid→DB cylinders via ellipse+rect caps), `network{nodes[]}`, `er{entities[]}`, `custom{elements:[skeletons]}` → auto `updateScene` + `scrollToContent` fit. Auto-layout uses gap/grid arithmetic + index `aN` z-order + `expand_label` for all text.

**Export:** `canvas.static.toDataURL('image/png')` prefix + length for verification; `browser_screenshot` (CDP `Page.captureScreenshot`) for full viewport PNG; `Blob+URL.createObjectURL` for `.excalidraw` download.

Verified live: `flowchart 12 els / microservices 30 els / sequence 13 / network 14 / er 20 / draw 2 els (rect+bound text) / export 47 els 94k dataUrl / save 47 els 26k JSON` all via hyprfast CLI in this session.

