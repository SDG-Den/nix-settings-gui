# Dux Shell v2 Settings and Edit Menu Internals

Analysis date: 2026-10-23

Analyzed tree: `~/repos/dux-shell`

Commit: `479b1434de3fb5077c2471072e2155e177ce14c5`

Tag/describe: `v2.0.0-5-g479b1434d`

Scope: the frontend QML that implements the settings and edit menus for the
control center, bar widgets, dashboard, weather surfaces, and launcher. Backend
Go code and service IPC are only described where the QML depends on them.

Caveat: `quickshell/DCommon` is a symlink to the `dux-qml-common` git submodule,
and the submodule is not checked out in this tree. Files such as
`DContextMenu.qml`, `DPopout.qml`, `DBottomSheet.qml`, `DEditableGrid.qml`,
`DGridEditChrome.qml`, `DReorderList.qml`, and `DPanelResizer.qml` are therefore
not readable here. Their contracts below are reconstructed from call sites.
Anything about their internals is explicitly marked as inferred.

---

## 1. Executive model

Dux Shell does not implement menus as a single global menu system. It uses five
distinct interaction primitives, and choosing the right one is the first design
decision to replicate:

1. **Page stack inside the settings window.** Persistent configuration. A single
   window with a sidebar, page history, aliases, and deep-link page IDs.
2. **Layer-shell context menus.** Short-lived command menus attached to a
   specific widget or selected result. These are separate surfaces, not widgets
   inside the page.
3. **Popouts.** Long-lived panels anchored to a bar widget or screen edge, such
   as the control center, dashboard, battery, VPN, clock, and launcher surfaces.
4. **In-surface edit overlays.** A grid edit mode with chrome handles, drag
   placeholders, a trash strip, and an action strip. The content stays in the
   popout; only the editing affordances are added.
5. **In-surface sheets/dialogs.** Bottom sheets and centered dialogs for "add
   widget", per-widget options, calendar details, and confirmation-style work.

The important architectural split is:

- **Runtime state and editor state are separate.** Normal mode renders and acts
  on settings. Edit mode overlays the same content with generic edit chrome.
- **Persistent values and option defaults are separate.** Widget instances store
  only overrides. A resolver combines instance values, type defaults, and global
  settings every time a menu is opened or a widget is rendered.
- **Menu contents are built from data.** `CcMenu` takes an array of menu item
  descriptors. `DContextMenu` takes either `menuItems` or `customContent`.
  Dashboard options are generated from option specs. Launcher actions are built
  from the selected result type. There is almost no hand-built menu tree.
- **A generic grid owns geometry, drag, resize, and commit.** Control center and
  dashboard do not implement their own packing. They provide `sourceItems`, a
  `slotLayout`, a `commitHandler`, and per-slot components. The shared
  `DEditableGrid` drives drag and resize.
- **Settings navigation is routed by page ID.** `SettingsModal.navigateTo`
  pushes onto a history stack, `SettingsContent` materializes a `StackView`
  page, and pages receive `parentModal` so they can navigate further.

For a GTK replication, the equivalent split is:

- A settings window with `AdwNavigationSplitView` or a sidebar plus `GtkStack`.
- A page registry with stable IDs, aliases, history, and deep links.
- `GtkPopover` for context menus and small option menus.
- A separate window or anchored panel for popouts.
- A reusable edit-session controller around any collection editor.
- A resolver layer for defaults plus overrides, not mutable UI state.

---

## 2. Surface topology

### 2.1 Popout entry points

Bar widgets create and own popout targets indirectly. The control center button
does not call a menu API. It is a `BasePill` whose click is handled by
`BarPill.handlePress` (`Modules/SurfaceWidgets/BarPill.qml:129-151`). The generic
pill:

- emits `rightClicked` for a secondary click,
- emits `pressedAt`,
- calls `popoutTarget.setBarContext` and `positionPopout`,
- then emits `clicked`.

Concrete widgets attach behavior to `clicked`, `rightClicked`, or expose a
method registered with `BarWidgetService`. The control center button is wired by
the bar host, not by the button file itself.

`PopoutService` owns the actual popout objects. Examples:

- `openControlCenter`, `toggleControlCenter`, `closeControlCenter`
  (`Services/PopoutService.qml:233-258`).
- `openDankDash`, `toggleDankDash`, `closeDankDash`
  (`Services/PopoutService.qml:351-395`).
- `openDankDashEditor` opens the dashboard directly in edit mode
  (`Services/PopoutService.qml:426-452`).
- `openSettingsWithTab` lazily creates or reveals the settings window and calls
  `showWithTabName` (`Services/PopoutService.qml:594-612`).

The island uses the same feature components but a different host. `DIsland`
maps an activity ID to a controller call in `openActivityOn`
(`Modules/DIsland/DIsland.qml:140-162`). `"weather"` maps to
`requestWeather(false)`. The island stores its own expanded/compact state in
`IslandController` (`Modules/DIsland/IslandController.qml:17-24`,
`717-765`).

### 2.2 The settings window

`SettingsModal` is a floating window, not a popout. It owns:

- `currentPage`, `pageHistory`, `currentTabIndex`, `currentParentId`,
  `command canGoBack` (`Modals/Settings/SettingsModal.qml:36-42`).
- `setPage`, `navigateTo`, `goBack`, `setTabIndex`, `showWithTabName`,
  `showKeybindsSearch`, `openPluginSettings`
  (`Modals/Settings/SettingsModal.qml:82-152`).
- compact mode with a hideable menu (`menuVisible`, `toggleMenu`)
  (`Modals/Settings/SettingsModal.qml:48-49`, `166-170`).

`SettingsContent` turns page IDs into QML:

- `pageFiles` maps stable IDs to files (`Modals/Settings/SettingsContent.qml:499-559`).
- `pagesWithParentModal` lists pages that need the navigation host injected
  (`Modals/Settings/SettingsContent.qml:561`).
- `_propertiesFor` injects `parentModal` and plugin IDs
  (`Modals/Settings/SettingsContent.qml:192-209`).
- `_syncPages` computes the common prefix of the current page path and the
  existing `StackView`, then pops, replaces, or pushes
  (`Modals/Settings/SettingsContent.qml:224-257`).
- `SettingsTabs` is the route table: IDs, aliases, parent/child relationships,
  hidden leaves, hub pages, plugin pages, and search metadata
  (`Common/SettingsTabs.qml:11-220`, resolver at `722-861`).

Important behaviors to copy:

- `navigateTo` remembers focus and appends the old page to history.
- `goBack` pops history, then falls back to the parent page.
- A page can be hidden from the sidebar but still reachable by ID.
- A hub page with one visible leaf resolves directly to that leaf.
- `parentModal` is the navigation capability object. Pages do not know about
  the window itself.
- Settings pages can close the source popout before navigating. For example,
  `CcSettingsButton` closes the control center and opens a settings tab
  (`Modules/ControlCenter/Widgets/CcSettingsButton.qml:15-18`), and the weather
  tab closes the dashboard before opening weather settings
  (`Modules/DDash/WeatherTab.qml:30-37`, `302-309`).

### 2.3 Context menus, popovers, and sheets

`CcMenu` is a thin local wrapper around `DContextMenu`:

- It accepts an array of `{ iconName, label, enabled, destructive, action }`.
- It filters invisible entries.
- It maps descriptors to `DContextMenu.menuItems`.
- It computes an anchor: align the menu's right edge to the button for LTR,
  align left in RTL, place below if it fits, otherwise above
  (`Modules/ControlCenter/Widgets/CcMenu.qml:21-58`).
- It restores focus to the anchor when the menu closes
  (`Modules/ControlCenter/Widgets/CcMenu.qml:60-67`).

`WidgetConfigOverlay` uses `DContextMenu.customContent` instead of `menuItems`.
It is the per-tile options "menu" for the control center
(`Modules/ControlCenter/Components/WidgetConfigOverlay.qml:82-195`). It does not
use a generic action list because rows contain embedded dropdowns, toggles, and
plugin navigation.

`LauncherContextMenu` is another `DContextMenu` subclass. Its `menuItems` is a
computed property that branches on the result type
(`Modals/DLauncherV2/LauncherContextMenu.qml:35-113`). It positions itself
relative to the launcher modal or host and disables the parent handler while
open (`149-165`).

`CcSheetDialog` is an in-popout centered dialog with a scrim, a focus scope,
Escape dismissal, an optional backdrop blur, and a scrollable content column
(`Modules/ControlCenter/Widgets/CcSheetDialog.qml:1-218`). It is used for
dashboard card options, calendar event details and editors, and per-widget
options in the weather/dashboard widget grid.

`DBottomSheet` is used by `CcWidgetSheet` for "Add widget" in the control
center (`Modules/ControlCenter/Components/CcWidgetSheet.qml:11-301`). It is a
bottom sheet with search, categories, and a preview grid.

The layer namespace string on each `DContextMenu` is important. Examples:

- `"dms:control-center-menu"` (`CcMenu.qml:43`)
- `"dms:control-center-widget-options"` (`WidgetConfigOverlay.qml:84`)
- `"dms:launcher-context-menu"` (`LauncherContextMenu.qml:32`)

This suggests each menu owns a named layer surface. A replication does not need
layer-shell, but it should preserve the idea that context menus are transient
top-level surfaces with their own focus and dismissal lifecycle.

---

## 3. Shared UI and data primitives

### 3.1 Settings page building blocks

`SettingsPage` is a `DFlickable` whose default content goes into a centered
column with a maximum width (`Modules/Settings/Widgets/SettingsPage.qml:1-31`).

`SettingsCard` is a titled column that registers itself with settings search:

- `tab`, `tags`, `settingKey`.
- optional `collapsible`/`expanded`.
- `headerActions` is forwarded to the section label.
- `SettingsSearchRegistration` registers the card (`SettingsCard.qml:1-75`).

`SettingsRow` is the base row control:

- `title`, `subtitle`, `iconName`, `clickable`, `showChevron`.
- `leading`, `body`, `trailing` aliases.
- `resetStore`, `resetKeys`, `modified`, `resetRequested`.
- keyboard activation on Space/Enter.
- focus tint and focus ring.
- `settingKey` search registration.
  (`Modules/Settings/Widgets/SettingsRow.qml:1-240`)

Specialized rows:

- `SettingsToggleRow` (`SettingsToggleRow.qml:1-60`).
- `SettingsDropdownRow`.
- `SettingsButtonGroupRow`.
- `SettingsSliderRow`.
- `SettingsNavRow`.
- `SettingsReorderRow`.
- `SettingsDivider`.
- `SettingsGroup`.

The recurring pattern is that a row's visual state comes from the current value,
and its change handler writes through a store. The store interface is often
`get`, `set`, `isDefault`, `resetToDefault`. This is visible in
`BarWidgetTab.qml:38-118`.

### 3.2 Option descriptors

Dashboard option descriptors are the most reusable menu/option model in the
tree. `DashRegistry` provides constructors:

- `toggle(key, text, def, description)`.
- `choice(key, text, def, choices)`.
- `number(key, text, def, min, max, step, unit)`.
  (`Modules/DDash/DashRegistry.qml:37-68`)

Descriptors can carry:

- `def`, `choices`, `min`, `max`, `step`, `unit`.
- `description`.
- `cardOnly`, meaning only valid for a dashboard card.
- `settingsOnly`, meaning only shown in the settings page, not the in-place
  options sheet.

`DashOptionRow` renders a descriptor according to `type`
(`Modules/DDash/Overview/DashOptionRow.qml:8-82`):

- toggle -> `DToggle`
- choice -> `DDropdown`
- number -> `DNumberStepper`

The value resolver is a pure function:
`Modules/DDash/utils/options.js:3-15`. It only accepts stored values of the right
type or range; otherwise it returns the descriptor default. This means stored
configuration can be partially invalid without breaking the UI.

### 3.3 Reorder lists

`SettingsReorderList` is a `DReorderList` that finds its parent flickable and
marks itself as a settings row
(`Modules/Settings/Widgets/SettingsReorderList.qml:1-13`).

`SettingsReorderRow` adds:

- a drag handle,
- Ctrl+Up/Ctrl+Down keyboard movement,
- drag state styling,
- shared group key for cross-list drag.
  (`Modules/Settings/Widgets/SettingsReorderRow.qml:1-75`)

`SettingsReorderGroup` is a `DReorderGroup` and receives `onTransferred` to move
an item across lists (`Modules/Settings/Widgets/SettingsReorderGroup.qml:1-3`,
used in `WidgetsTab.qml:64-69`).

The key design is that reorder lists emit an index order, not item objects.
`WidgetsTab` then maps indices back to its model and persists the reordered
array (`WidgetsTab.qml:668-671`, `760-763`, `794-797`).

### 3.4 Editable grid contract

The shared grid is not in this checkout. From call sites, its contract is:

- `sourceItems`: array of serializable item descriptors.
- `slotLayout`: precomputed packed layout with `slots`, `rowUnit`, `rows`, etc.
- `packer`, `placementOrder`, `gravity`, `rowStep`.
- `tileModel` and a Repeater of `DEditableGridSlot`.
- `draggingSourceIndex`, `sourceItems`, `heldOutside`, `hoverGroup`,
  `externalItem`, `committedItems()`, `previewExternal()`,
  `cancelInteraction()`, `clearExternal()`.
- `previewSize(index, changes)` for live resize preview.
- `commitHandler(items)` to persist the final layout.
- `sizePreview` and `dragging` state.

Control center (`Modules/ControlCenter/Components/CcTileGrid.qml:10-115`) and
dashboard card grid (`Modules/DDash/Overview/DashCardGrid.qml:11-298`) both use
it. They differ only in:

- how source items are derived,
- how sizes are clamped,
- what the drop/group rules are,
- what commit does.

`DGridEditChrome` is attached per slot or per panel. From call sites it supports:

- resize handles, `resizing`, `dragging`, `sizeText`, `atDefault`.
- `removable`, `hasOptions`.
- `onRemoveRequested`, `onOptionsRequested`.
- `onResizeStarted`, `onResizeMoved`, `onResizeEnded`, `onResizeCanceled`.
- corner and edge behavior, `cornerRadii`, `buttonSize`, `iconSize`.

This is the generic "edit chrome" that makes every tile or card editable without
each widget implementing handles.

### 3.5 Persistence model

`SettingsData` exposes each setting as a property with a default from
`SettingsSpec.js`, and calls `saveSettings()` on change. Examples:

- `controlCenterWidgets`, `controlCenterColumns`
  (`Common/settings/SettingsSpec.js:542-663`).
- `dashCards`, `dashOptions`
  (`Common/settings/SettingsSpec.js:873-904`).
- `barConfigs` (`Common/settings/SettingsSpec.js:1512-1586`).

Session-scoped user choices live in `SessionData` and `SessionStore.js`:

- `hiddenApps`
- `appOverrides`
- `dockPins`
- launcher history and last mode
  (`Common/SessionData.qml:253-277`, `1614-1662`).

Widget type defaults live in `Common/settings/BarWidgetDefaults.js`. The resolved
option accessor is:

```js
function option(widgetType, data, key) {
    if (data && data[key] !== undefined)
        return data[key];
    var defaults = DEFAULTS[widgetType];
    return defaults ? defaults[key] : undefined;
}
```

(`Common/settings/BarWidgetDefaults.js:301-306`)

That function is exposed as `SettingsData.widgetOption`
(`Common/SettingsData.qml:2473-2479`).

---

## 4. Control center

Primary files:

- `Modules/ControlCenter/ControlCenterPopout.qml`
- `Modules/ControlCenter/ControlCenterContent.qml`
- `Modules/ControlCenter/Components/CcTileGrid.qml`
- `Modules/ControlCenter/Components/CcTileSlot.qml`
- `Modules/ControlCenter/Components/CcEditActions.qml`
- `Modules/ControlCenter/Components/CcWidgetSheet.qml`
- `Modules/ControlCenter/Components/WidgetConfigOverlay.qml`
- `Modules/ControlCenter/Components/CcDetailPage.qml`
- `Modules/ControlCenter/Components/QuickTilesGroup.qml`
- `Modules/ControlCenter/Models/WidgetModel.qml`
- `Modules/ControlCenter/utils/widgets.js`
- `Modules/ControlCenter/CcMetrics.qml`

### 4.1 Popout state

`ControlCenterPopout` is a `DPopout`. It owns:

- `expandedSection`: the currently open detail page, empty for the tile grid.
- `pendingSection`: a section to open when the popout becomes visible.
- `triggerScreen`, `editMode`.
- `gridColumnCap`, `gridColumns`, `sheetContentWidth`, `availableHeight`.
- modals and nested loaders for power menu, color picker, accounts.
  (`ControlCenterPopout.qml:25-35`)

When shown, it applies `pendingSection`, focuses content, and handles cleanup
when hidden (`137-151`). `collapseAll()` sets `expandedSection = ""`
(`103-105`). `onPopoutClosed` forces `editMode = false` and calls
`collapseAll()` (`88-93`).

Opening settings from the control center is a deferred close-and-reopen:

1. `openSettings()` sets `requestedWindow = "settings"` and closes the popout
   (`39-42`).
2. `completeWindowClose` runs after the close animation
   (`56-67`).
3. A `DeferredAction` calls `PopoutService.focusOrToggleSettings()` or
   `PopoutService.openSettingsWithTab(...)` (`69-86`).

This avoids trying to open a window while the popout is still dismissing.

### 4.2 Content and edit state

`ControlCenterContent` is the actual menu host. Its state is the heart of the
control center editing model:

- `pageOpen`: a detail page is open.
- `editRoom`, `chromeHeight`, `stripAbove`: layout reservations for edit chrome.
- `widgetSheetOpen`, `widgetSheetRequested`.
- `placedWidgetIds`: a stable string of placed widget IDs used to refresh the
  edit button.
- `gridDragWidget`, `overTrash`, `dragRoom`, `heldGridHeight`.
- `editState`: `JSON.stringify({ widgets, columns })`.
- `editSnapshot`, `undoBase`, `undoStack`, `undoLimit = 50`, `undoBatching`.
- `editIdle`: not dragging, not resizing, not in a member drag.
- `canUndo`: edit mode, non-empty undo stack, idle, no sheet, no detail page.
  (`ControlCenterContent.qml:25-90`)

Edit session lifecycle:

- On entering edit mode: collapse all pages, take `editSnapshot = editState`,
  set `undoBase = editState`, clear undo stack, focus the content
  (`342-356`).
- On leaving edit mode: cancel panel resize, clear snapshot and undo base, reset
  widget sheet request (`347-353`).
- Every change to `editState` while in edit mode pushes the previous `undoBase`
  onto the undo stack, capped at 50 (`214-219`).
- `batchEdit(change)` suppresses intermediate states and pushes one undo entry
  (`221-231`).
- `applyEditState` writes both `controlCenterWidgets` and
  `controlCenterColumns`, but only when different (`186-194`).
- `cancelEdit` disables edit mode and restores the snapshot (`196-201`).
- `undoEdit` pops the stack, applies the previous state, and suppresses re-entry
  into the undo mechanism (`203-212`).

This is a full transactional editor. Dashboard edit mode is not; see section 6.

### 4.3 Widget model and default data

`WidgetModel` owns:

- The fixed component map from widget ID to tile component
  (`componentForWidget`, `140-187`).
- `baseWidgetDefinitions`, the catalog used by "Add widget"
  (`201-443`).
- Plugin widget discovery by instantiating a plugin component and checking for
  `ccWidgetIcon` (`getPluginWidgets`, `445-494`).
- Built-in plugin loaders for VPN, CUPS, Tailscale, display profiles
  (`20-71`).
- Thin wrappers to `WidgetUtils` for add, remove, layout, option, reset, clear
  (`496-523`).

The persisted array is `SettingsData.controlCenterWidgets`. A typical entry:

```json
{
  "id": "battery",
  "enabled": true,
  "w": 4,
  "h": 1,
  "col": 0,
  "row": 2,
  "batteryStyle": "icon",
  "batteryColorMode": "level"
}
```

Fields:

- `id`: widget type.
- `enabled`: reserved for hidden entries; `isShown` also filters battery and disk
  by hardware availability (`utils/widgets.js:60-69`).
- `w`, `h`: size in grid steps.
- `col`, `row`: packed position. Missing values mean "unplaced" and are handled
  by the packer (`isUnplaced`, `71-73`).
- `small`: half-row visual treatment for one-row widgets.
- `group`: parent quick-tiles group ID.
- `instanceId`: required for widgets that allow multiple instances.
- widget-specific options such as `batteryStyle`, `mountPath`, `deviceName`.

`WidgetUtils` is the source of truth for:

- option-capable IDs: `OPTION_IDS` (`diskUsage`, `brightnessSlider`,
  `idleInhibitor`, `user`, `battery`, `quickTiles`) plus plugins
  (`hasOptions`, `75-77`).
- grouping: `GROUP_ID = "quickTiles"`, `groupIds`, `inGroup`, `groupMembers`
  (`31-45`).
- edit button repair: `ensureEditButton` inserts a missing edit tile
  (`51-58`).
- add defaults and unique IDs (`90-161`).
- remove and ungroup (`163-179`).
- `setOption` and `setLayout` (`211-223`).
- reset and clear (`225-231`).

Note that reset and clear are built on `SettingsData.resetToDefault`, so they
rewrite the whole setting, not just the visible array.

### 4.4 Normal mode interactions

`ControlCenterContent` creates:
- `WidgetModel`.
- A `DFlickable` containing a `Column` with the tile grid
  (`417-486`).
- `CcTileGrid` with callbacks for expand, remove, config, color picker, close,
  settings, edit, accounts, lock, power (`445-483`).

Tile behavior is in `CcTile` and the tile-specific implementations:

- `CcTile.activate()` chooses between expand and click
  (`Widgets/CcTile.qml:133-141`).
- Long press calls `expand()` (`219`).
- Clicking a tile that has a detail page calls
  `grid.expandClicked(widgetData)` (`CcTileLoader.qml:88-96`), which becomes
  `ControlCenterContent.openWidgetPage`.
- `openWidgetPage` maps a widget instance to a section string with
  `sections.sectionFor` and pushes the current section onto `pageHistory`
  (`ControlCenterContent.qml:113-142`).

`sections.js` handles duplicate instances:

- `diskUsage_<instanceId>` and `brightnessSlider_<instanceId>`.
- `sectionFor(widgetData)` appends the instance ID for those two types.
- `widgetForSection` and `updateWidgetForSection` use that suffix to find and
  update the right instance (`Modules/ControlCenter/utils/sections.js:1-36`).

`CcDetailPage` is the detail dialog:

- It maps section IDs to detail components (`_componentFor`, `101-129`).
- It animates in and out, handles Escape and background click, and forwards
  `mountPathChanged`, `deviceNameChanged`, codec/port selector requests, dismiss,
  and close signals back to the host (`131-355`).
- It injects instance-specific values into `DiskUsageDetail` and
  `BrightnessDetail` (`397-414`).
- `PluginDetailShell` handles plugin and built-in plugin pages (`423-429`).

Detail pages can expose `headerActions`; `CcDetailPage` moves that item into the
dialog header (`302-306`). This is how a detail page gets a settings gear that
takes the user to the corresponding settings page.

### 4.5 Entering and leaving edit mode

Entry points:

- The `edit` action tile: `ActionTile` calls `host.editRequested()`
  (`Widgets/ActionTile.qml:18-33`), and `CcTileSlot.onClicked` calls
  `grid.editRequested()` for `EDIT_ID` (`CcTileSlot.qml:70-73`).
- Keyboard: F2 or Ctrl+E while visible and not on a page
  (`ControlCenterContent.qml:319-324`).
- The edit chrome and action strip appear only in edit mode
  (`358-388`, `488-515`).

Leaving:

- Finish button: `host.editMode = false` (`513`).
- Discard button: `cancelEdit()` restores the snapshot (`196-201`, `514`).
- Escape: closes sheets, overlay, detail page, then edit mode, then the whole
  panel (`278-305`).
- Closing the popout forces `editMode = false`, but does not restore the
  snapshot. Only the explicit Discard restores state.

### 4.6 Edit action strip

`CcEditActions` is the strip at the bottom or top of the panel
(`Components/CcEditActions.qml`). It provides:

- Add widget.
- A "More" menu: Undo, Reset to default, Clear All.
- Discard.
- Finish.
- While dragging a removable widget, the strip becomes a trash target.

The action menu is a `CcMenu` with three items (`186-209`). Reset runs through
`batchEdit` so it becomes one undo entry. Clear is immediate but still enters the
undo stack through `onEditStateChanged`.

The strip geometry is non-trivial:

- It can be joined to the panel or float as a separate shoulder.
- `joined` controls whether it shares the panel surface and corners.
- `above` flips arcs and trash direction.
- `trashContains(scenePosition)` uses a half-plane test, not a rectangle, so a
  held tile stays held until it moves back into the panel
  (`55-67`).

If replicating the UX, keep the "drag outside the panel becomes trash" rule and
the one-way trash activation. It is less fiddly than a strict drop zone.

### 4.7 Drag, drop, resize, grouping, trash

`CcTileGrid` delegates packing to `GridUtils.packCards` and `shrinkRows`
(`CcTileGrid.qml:47-54`). It computes:

- `savedWidgets` from `SettingsData.controlCenterWidgets`.
- `groupIds` and `shownIndices`.
- `sourceItems`: only shown, non-grouped widgets with clamped sizes.
- `slotLayout`: packed layout with small-row shrinking.
- `packer`: live cell packing while dragging.

`CcTileSlot` handles:

- size constraints via `sizeSpec` and `clampSize`.
- small tile half-row bias.
- resize preview through `grid.previewSize`.
- slider special case: sliders cannot be both one column and one row; they grow
  to a valid size (`81-105`).
- edit chrome remove/config callbacks (`120-145`).

`dropFromGrid` is deliberately deferred with `Qt.callLater` because committing
inside the release handler would destroy the dragged item mid-signal
(`ControlCenterContent.qml:241-261`). The drop has two outcomes:

- drop into a quick tiles group: `WidgetUtils.moveToGroup`,
- drop outside the panel: `widgetModel.removeWidget`.

Grouping:

- `QuickTilesGroup` renders members in a row and handles drag-out and reorder
  (`Components/QuickTilesGroup.qml:79-128`).
- Dragging a member outside the group previews it as a grid item
  (`95-102`) and on drop places it with `placeFromGroup` (`121-127`).
- Dragging any groupable tile over a group and dropping calls `moveToGroup`
  (`ControlCenterContent.qml:246-255`).
- Deleting a group ungroups its members (`utils/widgets.js:163-179`).

Trash is not a separate model state. `overTrash` is derived from whether the
grid drag widget is removable and the pointer is outside any group
(`ControlCenterContent.qml:42-44`). `CcTileSlot` scales and translates the tile
into the trash position (`CcTileSlot.qml:43-55`).

Column resizing is also part of edit mode. `DPanelResizer` previews a column
count and commits `controlCenterColumns`
(`ControlCenterContent.qml:76-89`). `CcMetrics.columnPreview` overrides the
stored column count while previewing (`CcMetrics.qml:15-23`).

### 4.8 Per-widget option menu

`WidgetConfigOverlay` is opened by:

- long press on a tile whose edit chrome has options
  (`CcTileSlot.qml:75-79`),
- the options button on the edit chrome
  (`CcTileSlot.qml:144`).

It computes the widget data from the saved index, not from a copied object
(`WidgetConfigOverlay.qml:23-42`). It opens a `DContextMenu` with custom content:

- plugin settings row for `plugin_*` IDs (`100-110`),
- background toggle for user or group (`112-117`),
- battery style and color rows, including a battery nested in a group
  (`119-153`),
- user options from `DashRegistry.sheetOptionSpecs("user")` (`155-165`),
- disk mount-path toggle (`167-172`),
- idle inhibitor duration dropdown (`174-192`).

Brightness is special-cased: it opens the full detail page instead of the
overlay (`168-171`).

Option writes use `WidgetUtils.setOption(widgetIndex, key, value)`, which
replaces the whole widget entry immutably and saves it
(`WidgetConfigOverlay.qml:78-80`, `utils/widgets.js:211-219`).

The menu position is computed after propagation of `widgetIndex` with
`Qt.callLater`, because the menu height is only final after the content is
built. It prefers above the anchor and falls back below
(`WidgetConfigOverlay.qml:46-67`).

### 4.9 Add widget sheet

`CcWidgetSheet` is a `DBottomSheet`:

- `widgets` passed in are the addable definitions.
- `addable` filters `enabled !== false`.
- `matches` uses `WidgetUtils.filterWidgets` over text, description, and ID.
- Categories are fixed: User, Network, Audio, Display, System, Plugins.
- Collapsed categories persist in `CacheData.controlCenterCollapsedCategories`.
- The sheet height is 85 percent of available.
- Adding calls `chosen(widgetId)`, and the host calls `widgetModel.addWidget`
  (`CcWidgetSheet.qml:54-67`, `258-300`;
  `ControlCenterContent.qml:579-606`).
- While the sheet is open, edit actions are disabled
  (`ControlCenterContent.qml:498`).

The "Add widget" source list is:

```js
widgetModel.baseWidgetDefinitions.concat(widgetModel.getPluginWidgets())
```

filtered by `allowMultiple` or not already placed
(`ControlCenterContent.qml:595-599`). This is why most widgets cannot be added
twice, while brightness, disk, and quick tiles can.

### 4.10 Keyboard and focus

- Escape priority: codec selector, port selector, widget sheet, config overlay,
  detail page, edit mode, close panel (`ControlCenterContent.qml:278-317`).
- F2/Ctrl+E enters edit mode.
- Ctrl+Z undoes while `canUndo`.
- The detail page implements Tab/Backtab movement within the dialog and returns
  focus to the invoking item on dismissal
  (`CcDetailPage.qml:67-92`, `536-544`).
- The edit strip menu and widget options menus are keyboard navigable because
  `DContextMenu.keyboardNavigable` is true.

### 4.11 End-to-end flows

**Open a detail page.** Click a tile with a detail page. `CcTile` emits
`expandClicked`. `CcTileLoader` forwards to `CcTileGrid.expandClicked`.
`ControlCenterContent.openWidgetPage` computes the section and navigates.
`CcDetailPage` loads the component and focuses it. Back pops `pageHistory` or
collapses.

**Edit a tile option.** Enter edit mode, long-press the tile or press its options
button. `WidgetConfigOverlay` opens as a context menu. A toggle writes through
`WidgetUtils.setOption`. The menu closes, focus returns to the chrome, and the
new value is visible immediately because the tile reads the saved entry.

**Add a widget.** Edit mode, Add widget, bottom sheet. Search or pick a category.
Choosing calls `addWidget`, which appends a default instance and saves. The sheet
closes.

**Reorder.** Edit mode, drag a tile. The grid packs live. Dropping in the grid
calls `commitHandler` with the new layout. Dropping on a group calls
`moveToGroup`. Dropping outside the panel removes the widget unless it is the
edit action tile. All changes are reflected in `editState`, so undo works after
the drag.

**Delete.** Drag to trash, or use the chrome remove button. Remove splices the
saved array and ungroups members if the removed item is a group.

**Discard.** The snapshot taken when edit mode started is restored. This is a
real rollback, not just a reload.

---

## 5. Bar widgets

Primary files:

- `Modules/DBar/WidgetModel.js`
- `Modules/SurfaceWidgets/SurfaceWidgetHost.qml`
- `Modules/SurfaceWidgets/BarPill.qml`
- `Common/BarWidgetCatalog.qml`
- `Common/settings/BarWidgetDefaults.js`
- `Modules/Settings/WidgetsTab.qml`
- `Modules/Settings/WidgetsTabSection.qml`
- `Modules/Settings/BarWidgetTab.qml`
- `Modules/Settings/WidgetSelectionPopup.qml`
- `Modules/Settings/BarWidgetOptions/*`

### 5.1 Runtime composition

A bar is a list of configs in `SettingsData.barConfigs`. Each config has:

- `leftWidgets`, `centerWidgets`, `rightWidgets`,
- appearance and behavior fields,
- per-section overflow fields such as `leftOverflowMode` and
  `leftOverflowPosition`
  (`Common/settings/SettingsSpec.js:1512-1586`).

Widget lists can contain strings (legacy shorthand) or objects. `WidgetModel.normalize`
converts strings to `{ widgetId, id, enabled }` and assigns a stable runtime ID
of `widgetId + "_" + index` (`Modules/DBar/WidgetModel.js:3-18`). The component
map translates widget IDs to QML components (`20-54`).

`SurfaceWidgetHost` loads the component and injects:

- config and geometry,
- screen, section, axis, orientation,
- `widgetData`, `surfaceContext`, `hostContext`,
- `widgetInstanceId`,
- plugin service and popout service when the widget declares those properties.

It also registers the widget with `BarWidgetService` and unregisters it on
destruction (`Modules/SurfaceWidgets/SurfaceWidgetHost.qml:79-167`).

`BarWidgetService` maintains a registry keyed by screen, surface, and instance.
It resolves a widget by target, resolves an IPC widget by focused screen and
last used bar, and provides `triggerWidgetPopout`
(`Services/BarWidgetService.qml:79-122`, `148-181`, `255-295`).

Click behavior is not centralized. `BarPill` emits `clicked` and
`rightClicked`, and the concrete widget decides what those mean. Some widgets
use `popoutTarget.toggle()`, some emit a named signal such as `clockClicked`,
and some do nothing. `triggerWidgetPopout` has a compatibility map for the
named-signal widgets (`BarWidgetService.qml:267-292`).

The weather bar widget specifically is display-only. `Weather.qml` renders an
icon and temperature and has no click or popout target
(`Modules/DBar/Widgets/Weather.qml:1-75`). Configuration is reached through the
settings page, not through the widget.

### 5.2 Bar widget settings hub

`WidgetsTab` is the hub page for `dankbar_widgets`.

State:

- `selectedBarId` from `SettingsUiState`.
- `selectedBarConfig`.
- `hasMultipleBars`.
- `highlightedId`, `highlightedSection` for keyboard operation.
- `baseWidgetDefinitions`: `BarWidgetCatalog.widgets` plus plugin variants.
- `widgetAvailability`: per-widget enabled/warning rules.
  (`WidgetsTab.qml:12-130`)

It has three section components, one per bar section:

- `WidgetsTabSection` for left, center, right.
- Each section gets its items from `getItemsForSection`, which merges the widget
  definition with the saved instance data and defaults `enabled` to true
  (`WidgetsTab.qml:505-528`).
- Center is hidden when the island owns the center (`41-54`, `746-779`).

`WidgetsTabSection` renders:

- A section label with overflow controls and centering controls.
- An optional overflow settings card.
- A reorder list.
- An "Add widget" button.

Each reorder row shows:

- drag handle,
- icon,
- title and description,
- warning icon and tooltip,
- chevron if configurable,
- spacer size stepper for spacers,
- enabled toggle,
- remove button.
  (`WidgetsTabSection.qml:130-232`)

The row click behavior is:

- spacer and separator are not configurable; their size is edited inline.
  `BarWidgetCatalog.configurable` returns false for them
  (`Common/BarWidgetCatalog.qml:274-277`).
- plugin-backed entries navigate to the plugin settings page.
- all other configurable entries navigate to the generic `bar_widget` page.
  (`WidgetsTab.qml:465-503`)

### 5.3 Add widget picker

`WidgetSelectionPopup` is a `WidgetPickerWindow`, a floating window with its own
search, keyboard navigation, and list delegate
(`Modules/Settings/WidgetSelectionPopup.qml:1-107`).

The picker source is `getWidgetsForPopup`, which:

- filters disabled plugins and unavailable widgets,
- filters singleton widgets already present,
- disables `island` when the current bar is an island or another island already
  covers the display (`WidgetsTab.qml:295-318`).

On selection, `addWidgetToSection` calls `SettingsData.addBarWidget`
(`WidgetsTab.qml:320-324`). That function:

- appends an entry with `enabled: true`,
- initializes known option defaults (`spacer.size`, `gpuTemp` GPU fields,
  `diskUsage.mountPath`),
- has special handling for the island widget (ownership and eviction)
  (`Common/SettingsData.qml:2497-2534`).

### 5.4 Reorder and cross-section movement

Within a section:

- `SettingsReorderList` emits indices.
- `WidgetsTab` maps indices to the current array and saves the whole array
  (`WidgetsTab.qml:668-671`, `760-763`, `794-797`).

Across sections:

- `SettingsReorderGroup.onTransferred` calls `moveWidget`.
- `moveWidget` removes from the source array and inserts into the target array at
  the requested index, saving both keys in one `updateBarConfig` call
  (`WidgetsTab.qml:377-401`).

Keyboard:

- Up/Down moves the highlight.
- Ctrl+Up/Ctrl+Down moves the highlighted widget within the section.
- Ctrl+Left/Ctrl+Right moves it across sections.
- Space toggles enabled.
- Enter opens settings.
  (`WidgetsTab.qml:132-183`)

The code uses indexes rather than widget IDs for identity, because the same
widget type can appear more than once. This is a key detail for replication:
do not key the editor by type ID.

### 5.5 The per-widget settings page

`BarWidgetTab` is a parameterized page. It gets its target from
`SettingsUiState`:

- `selectedDockId`, `selectedDockWidgetId` for dock-hosted widgets,
- `selectedBarId`, `selectedWidgetSection`, `selectedWidgetIndex` for bar
  widgets,
- `selectedWidgetTitle`, `Description`, `Icon` for the page title.
  (`Common/SettingsUiState.qml:8-22`)

It resolves the target entry in two ways:

- dock: find by ID in the selected dock config,
- bar: read `config[section + "Widgets"][index]`, normalizing a string to an
  object (`BarWidgetTab.qml:21-33`).

It then defines a store adapter with `get`, `set`, `isDefault`, and
`resetToDefault` (`38-43`). The default lookup is:

- `overflowMode` defaults to `"section"` for bar widgets,
- dock-specific defaults for `appsDock`,
- otherwise `SettingsData.widgetDefaults(widgetType)[key]`.
  (`83-89`)

The value lookup is `SettingsData.widgetOption(widgetType, entry, key)`
(`91-97`). Set writes:

- `SettingsData.updateDockWidget` for dock-hosted,
- `SettingsData.updateBarWidget` for bar-hosted.
  (`99-106`)

The page always shows:

- a weather navigation card when the widget is weather,
- enable/disable row,
- placement dropdown where applicable,
- background toggle where applicable
  (`134-180`).

It then loads an options file if one exists:

```js
optionsLoader.setSource("BarWidgetOptions/" + file, { page: root });
```

(`BarWidgetTab.qml:120-132`)

`BarWidgetCatalog.optionsFile` maps widget types to option components
(`Common/BarWidgetCatalog.qml:239-260`). Weather has no options file, so its
only settings affordance in this page is the navigation card to the weather
settings page.

Example pattern: `ClockOptions.qml` uses `page.value` and `page.set` for each
control and adds a navigation row to `time_weather`
(`Modules/Settings/BarWidgetOptions/ClockOptions.qml:13-48`). The options page
knows nothing about bar config layout; it only talks to the adapter.

### 5.6 Plugins

A plugin entry in a bar section has `pluginId`. `WidgetsTab.configureWidget`
detects it and navigates to `SettingsTabs.pluginPrefix + pluginId`
(`WidgetsTab.qml:490-495`). The plugin page is loaded by
`SettingsContent._fileFor` and `_propertiesFor`
(`Modals/Settings/SettingsContent.qml:182-197`). This keeps plugin menus out of
the generic widget options page.

### 5.7 Bar-level right-click and configurable actions

Right-clicking the bar background is not a widget menu. The bar config has
`middleClickAction` and `rightClickAction` (`SettingsSpec.js:1571-1572`). These
are resolved by `DBarBody` (`Modules/DBar/DBarBody.qml:196-198`) and edited in
`DBarTab` (`Modules/Settings/DBarTab.qml:544-595`). The available actions are
bar-level commands, not widget configuration.

Widget right-click is widget-specific. For example:

- control center right-click toggles mute or mic mute depending on the sub-icon
  under the pointer (`Modules/DBar/Widgets/ControlCenterButton.qml:85-109`),
- launcher right-click toggles the overview (`LauncherButton.qml:37-45`).

There is no generic "right-click a bar widget to edit it" behavior. Editing is
done in the settings window.

### 5.8 Dock reuse

`BarWidgetTab` is shared with dock widgets by switching the store and the target
lookup. `DockWidgetsTab` sets `SettingsUiState.selectedDockId` and
`selectedDockWidgetId` and navigates to `bar_widget`
(`Modules/Settings/DockWidgetsTab.qml:115-118`). `BarWidgetTab` then reads the
dock config and writes with `updateDockWidget`. Option keys that belong to the
dock surface get a `bar` prefix when hosted on a bar
(`BarWidgetTab.qml:45-71`).

This is a strong pattern to copy: one editor UI, two storage adapters. It avoids
duplicating every option page for bar and dock.

### 5.9 Persistence summary

- Bar layout and options: `SettingsData.barConfigs`, saved by
  `updateBarConfig`.
- Widget entry values: stored directly on each entry object.
- Widget type defaults: `BarWidgetDefaults.DEFAULTS`.
- Runtime IDs: generated per index, not persisted as identity.
- Plugin settings: separate plugin settings storage and page.

### 5.10 End-to-end flow

1. Settings, Bar widgets.
2. Pick a bar if multiple.
3. Section list shows current order and enabled state.
4. Add widget opens a picker, choose a type, it is appended to that section.
5. Click a row to open `bar_widget`, which builds the adapter and loads the
   option component.
6. Toggling an option writes through `updateBarWidget` and saves the config.
7. Drag a row or use Ctrl+arrows to reorder, including across sections.
8. The bar re-renders from the same config array.

---

## 6. Dashboard

Primary files:

- `Modules/DDash/DDashPopout.qml`
- `Modules/DDash/DashTabHost.qml`
- `Modules/DDash/DashRegistry.qml`
- `Modules/DDash/OverviewTab.qml`
- `Modules/DDash/Overview/DashCardGrid.qml`
- `Modules/DDash/Overview/DashCardSlot.qml`
- `Modules/DDash/Overview/DashEditChrome.qml`
- `Modules/DDash/DashPageActions.qml`
- `Modules/DDash/Overview/DashEditControls.qml`
- `Modules/DDash/Overview/DashOptionsSheet.qml`
- `Modules/DDash/DashWidgetGrid.qml`
- `Modules/DDash/utils/cards.js`
- `Modules/DDash/utils/options.js`
- `Modules/DDash/utils/widgets.js`
- `Modules/Settings/DDashTab.qml`

### 6.1 Surfaces and tabs

`DDashPopout` is the dashboard popout. It owns:

- `dashVisible`, `triggerScreen`.
- `currentTabId`, `detailTabId`.
- `editMode`.
- `overviewFocusId`.
- derived `activeTabId`, `detailEntry`, `orderedTabIds`, `showTabs`,
  `showHeader`, `navigationEdge`, `verticalNavigation`.
  (`DDashPopout.qml:38-95`)

Tabs are rendered twice:

- A `DNavigationBar` for the top-level tabs, editable, with edit request
  (`618-637`).
- A `ScriptModel` of `DashRegistry.tabIds`, each host loading the tab component
  (`725-765`).

`DashTabHost` injects properties into the tab component: `entryId`, `live`,
`active`, `keyForwardTarget`, `contentViewport`, `parentPopout`, `dashHost`,
`transientSurfaceTracker`, `targetScreen`, `preferredFocusId`, `editMode`,
`rowBudget`, `columnCap`, and plugin fields
(`DashTabHost.qml:63-100`).

The same tabs are reused inside the island through `DashTabFace`
(`Modules/DIsland/Activities/DashTabFace.qml`), and the weather activity wraps
`WeatherTab` with it (`WeatherExpanded.qml:6-16`).

### 6.2 Dashboard registry and option scopes

`DashRegistry` is a singleton and the dashboard's schema:

- `builtins`: built-in tab/card definitions, with `tab`, `card`, and `options`.
- `pluginEntries`: plugin tabs/cards discovered from `PluginService`.
- `entries`: builtins plus plugins, with default card sizes and panel options
  merged in.
- `tabEntries`: stored order/enabled state merged with definitions.
- `tabIds`, `visibleTabIds`, `tabBarModel`.
- `cardEntries`, `placed`, `unplaced`.
  (`DashRegistry.qml:88-466`)

A tab definition can have:

- `tab.component`,
- `tab.async`,
- `tab.sizeToContent`,
- `card.component`, default size, min/max sizes,
- `options` as an array of descriptors.

Options have scope flags:

- `cardOnly`: do not show in a tab's options sheet.
- `settingsOnly`: only show in the settings page, not in the in-place sheet.
  Panel width and height use this flag (`panelOptions`, `80-86`).

`DashRegistry.sheetOptionSpecs` removes `settingsOnly` and, for tab scope,
`cardOnly` (`508-510`).

### 6.3 Dashboard edit mode

Entry points:

- F2 or Ctrl+E while the popout is visible (`DDashPopout.qml:496-501`).
- The edit affordance on the tab bar (`onEditRequested`, `625`).
- The edit affordance on the page title when tabs are hidden
  (`DashPageTitle`, `639-647`).
- `PopoutService.openDankDashEditor`, which sets `editMode = true` before
  showing the popout (`PopoutService.qml:426-452`).

Global behavior:

- `onActiveTabIdChanged` closes edit mode and dismisses options
  (`DDashPopout.qml:135-141`).
- `onPopoutClosed` sets `editMode = false` and clears `detailTabId`
  (`189-192`).
- Escape in edit mode exits edit mode before closing the popout
  (`440-446`).
- Tab/Shift+Tab cycle tabs when not in edit mode; in edit mode they move within
  the edit controls (`428-438`).

Unlike the control center, dashboard edits persist immediately. There is no
snapshot, discard, or undo stack. `commitHandler` writes `dashCards` or the tab's
widget layout as soon as the grid commits
(`DashCardGrid.qml:44-47`, `DashWidgetGrid.qml:127-134`). "Finish" only leaves
edit mode. "Reset" and "Clear" are immediate resets, with a two-step confirm in
the controls.

### 6.4 Overview grid

`OverviewTab` is the root of the default tab:

- `editMode`, `rowBudget`, `columnCap`, `preferredFocusId`.
- `blocksTabNavigation` includes card resizing, sheets, editor, and add menu.
- `handleKeyEvent` dismisses sheets/menus on Escape, then forwards to the grid.
- `openAddMenu(anchor)`.
  (`OverviewTab.qml:16-80`)

The grid:

- `sourceItems: DashRegistry.placed`.
- `slotLayout: GridUtils.packCards(...)`.
- `commitHandler: items => CardUtils.setLayout(items)`.
- `addableEntries`: unplaced card entries with a fitted default size.
  (`DashCardGrid.qml:34-47`)

Cards:

- `DashCardSlot` loads the card component from `DashRegistry.cardComponentFor`.
- It injects `entryId`, `transientSurfaceTracker`, and plugin fields.
- It binds `live` and `interactive`.
- Click opens the corresponding tab if the card has one.
- `DashEditChrome` provides remove, options, and resize.
  (`DashCardSlot.qml:51-156`)

Add card:

- `OverviewTab` has a `CcMenu` whose items come from `grid.addableEntries`.
- Choosing calls `CardUtils.addCard(id, w, h)`.
  (`OverviewTab.qml:111-120`)

Card options:

- `optionsRequested(cardId)` opens `DashOptionsSheet` with `tabScope = false`.
- The sheet shows `DashRegistry.sheetOptionSpecs(cardId)`.
- Each `DashOptionRow` writes through `DashRegistry.setOption`.
  (`OverviewTab.qml:99`, `122-126`; `DashOptionsSheet.qml:23-49`)

Calendar detail and editor are `CcSheetDialog` instances, not context menus
(`OverviewTab.qml:128-205`).

### 6.5 Page actions

`DashPageActions` combines the edit controls with per-tab behavior:

- `menuOpen` and `hasCustomActions` are derived from `CcMenu`.
- `focusTargets` come from the controls.
- `reset()`:
  - resets panel size if resizable,
  - overview resets cards,
  - other tabs call `tabItem.resetWidgets()`.
- `clear()`:
  - overview clears cards,
  - other tabs call `tabItem.clearWidgets()`.
- Custom actions are `tabItem.menuActions`.
- `DashEditControls` gets add/options/reset/clear/finish callbacks.
  (`DashPageActions.qml:9-79`)

`DashEditControls` renders the buttons, supports horizontal or vertical layout,
and implements the two-step confirm for reset and clear
(`DashEditControls.qml:60-189`). "Confirm" is an inline state: the first click
arms the button, the second executes. Escape cancels.

This is a useful pattern for destructive actions in a compact toolbar. The
control center instead relies on undo for clear and a single menu item for reset.

### 6.6 Tab options sheet

`DashOptionsSheet` is a `CcSheetDialog`:

- `entryId`, `tabScope`.
- `entry` from `DashRegistry.entry`.
- `specs` from `DashRegistry.sheetOptionSpecs`.
- optional nav row to a full settings page for weather, media, or wellbeing.
- "Reset to default" calls `DashRegistry.resetOptions(entryId)`.
  (`DashOptionsSheet.qml:9-72`)

For a tab, `tabScope = true`, so card-only options are removed. For a card,
`tabScope = false`, so card options remain. Panel width and height are not in
the sheet; they are edited by the panel chrome.

### 6.7 Dashboard widget grids (weather and other tab-local widgets)

`DashWidgetGrid` is a reusable tab-local widget editor. It is used by the weather
tab and can be used by any tab that exposes a `widgets` grid:

- `entryId`, `definitions`, `editMode`, `live`.
- `widgets = WidgetUtils.resolve(definitions, DashRegistry.widgetLayout(entryId))`.
- `addable = definitions.filter(d => !widgets.some(w => w.id === d.id))`.
- `save(items)` writes `dashOptions[entryId].widgets` via
  `DashRegistry.setWidgetLayout`.
- `resetWidgets()` deletes the custom layout.
- `clearWidgets()` saves an empty array.
- `openAddMenu(anchor)`.
- focus helpers.
  (`DashWidgetGrid.qml:16-125`)

The grid uses a `DEditableGrid` with `DEditableGridSlot` children. Each slot:

- resolves the widget spec,
- loads the component,
- injects `live`, `widgetId`, and `widgetOptions`,
- adds `DashEditChrome` with options only if the spec has options.
  (`DashWidgetGrid.qml:127-195`)

Options sheet:

- `CcSheetDialog`, title from the spec.
- `SettingsGroup` with `DashOptionRow` per option.
- Committing calls `root.updateWidget(index, { key: value })`, which calls
  `grid.commitChange` and therefore saves the whole widget layout.
  (`DashWidgetGrid.qml:219-257`)

`utils/widgets.js` resolves saved widget arrays:

- `LAYOUT_KEYS` are `id`, `w`, `h`, `col`, `row`; they are not treated as
  options.
- `resolve(definitions, saved)` filters duplicates, merges spec defaults with
  saved values, and applies `Options.value` per option.
  (`Modules/DDash/utils/widgets.js:4-35`)

### 6.8 Dashboard settings page

`DDashTab` is the persistent dashboard settings page:

- Nav cards for weather, media player, digital wellbeing.
- Tabs card:
  - position dropdown,
  - evenly spaced toggle,
  - reorder list of tabs,
  - per-row enable toggle,
  - edit button calling `PopoutService.openDankDashEditor(id, screen)`.
- Options cards for every entry with options, generated from `DashRegistry`.
  (`Modules/Settings/DDashTab.qml:19-204`)

The tab edit button is a deep link into the live editor, not a settings form.
This is a deliberate pattern: configuration that is easier to manipulate in
context opens the actual surface in edit mode.

### 6.9 Persistence and semantics

- Card layout: `SettingsData.dashCards`.
- Per-entry options: `SettingsData.dashOptions`, keyed by entry ID.
- Tab order and enabled state: `SettingsData.dashTabs`, managed by
  `getDashTabs`, `setDashTabOrder`, `setDashTabEnabled`, `resetDashTabs`
  (`Common/SettingsData.qml:562-620`).
- Tab-local widget layout: `dashOptions[entryId].widgets`.
- Panel size: `dashOptions[entryId].panelColumns` and `panelRows`.
- Reset semantics:
  - `resetDashCards` restores defaults and deletes `dashOptions.overview`.
  - `resetOptions` removes only keys that belong to the entry's option specs.
  - `resetPanelSize` restores default columns and rows.
  - `resetDashTabs` restores default tab list.

### 6.10 End-to-end flows

**Add a card.** Enter edit mode on Overview, Add widget, choose from `addable`.
The card is appended to `dashCards` with its fitted size. The grid packs it.

**Change a card option.** In edit mode, press the card options button. The sheet
shows card-scoped options. Changing one writes `dashOptions[cardId]`, deletes it
if it equals the default, and the card re-renders from `Card.options`.

**Resize a card or panel.** Drag the corner. `DashCardSlot` computes a fitted
size and calls `previewSize`; on end the grid commits. Panel resize uses
`DashMetrics.panelPreview` and commits `panelColumns` or `panelRows`.

**Edit a weather widget.** Enter edit mode on the weather tab. Each widget has
its own options. Changing the hourly widget's forecast style writes into the
weather tab's widget layout, not a global weather setting.

**Open the weather settings page.** Use the weather tab's Actions menu or its
empty-state Settings button. The dashboard closes and the settings window opens
on `weather`.

---

## 7. Weather

Weather is not one menu. It appears in five places, and each uses a different
part of the same model:

1. Dashboard overview weather card.
2. Dashboard weather tab.
3. Island weather activity.
4. Bar weather widget.
5. Weather settings page.

### 7.1 Overview card

`WeatherOverviewCard` is a dashboard `Card` with `entryId: "weather"`
(`Modules/DDash/Overview/WeatherOverviewCard.qml:9-57`). It reads
`DashRegistry.resolvedOptions("weather")`, which includes:

- `city` toggle,
- `readings` toggle,
- `tone` choice.
  (`DashRegistry.qml:177`)

It uses `WeatherService` reference counting: `addRef` on live, `removeRef` on
destruction (`WeatherOverviewCard.qml:59-72`). Clicking it opens the weather tab
because `DashCardSlot` sees `card.clickable` and emits `cardClicked`, which
`OverviewTab` maps to `tabRequested(cardId)` (`DashCardSlot.qml:111-119`,
`OverviewTab.qml:94-97`).

### 7.2 Weather tab layout

`WeatherTab` defines a `DashWidgetGrid` with these widget definitions:

- `conditions`: current conditions, default 2x2, options tone and city.
- `hourly`: hourly forecast, default 2x2, options forecast chart/cards.
- `sun`: sunrise/sunset, default 1x2, tone option.
- `moon`: moon position, default 1x2, tone option.
- `humidity`: metric, default 1x2, graphics and tone.
- `wind`: metric, default 1x2, graphics and tone.
- `daily`, `pressure`, `precipitation`, `visibility`, `uv`: defined but
  `enabled: false` in the default set.
  (`Modules/DDash/WeatherTab.qml:87-237`)

Each widget component receives `widgetOptions` from `DashWidgetGrid`, so the
same component can render different sizes and options. For example, the
conditions widget passes `widgetOptions.city` to `WeatherHero`, and hourly passes
`widgetOptions.forecast` to `WeatherForecastWidget`
(`WeatherTab.qml:239-277`).

The tab also owns:

- `available = WeatherService.weather.available`.
- `menuActions`: Refresh Weather and Settings.
- `blocksTabNavigation`.
- `syncWeatherRef` for live reference counting.
- `openAddMenu`, `resetWidgets`, `clearWidgets`, `handleKeyEvent`, `cycleFocus`
  (`WeatherTab.qml:14-85`).

If weather is unavailable, a `CcEmptyState` shows the last fetch error, a refresh
button, and a settings button (`274-311`).

### 7.3 Weather tab edit mode

The tab itself does not draw edit chrome. `DashWidgetGrid` does:

- `editMode` comes from `WeatherTab`, which receives it from `DashTabHost` or
  `DashTabFace`.
- Add uses a `CcMenu` built from `addable`.
- Each widget gets a `DashEditChrome` with remove, options, and resize.
- Options use `DashOptionsSheet` inside `DashWidgetGrid`.

In the dashboard, the tab edit mode is injected by `DashTabHost` from
`DDashPopout.editMode` (`DashTabHost.qml:86-87`). In the island, `DashTabFace`
owns its own `editMode` and propagates it to the tab
(`DashTabFace.qml:19`, `101-115`).

### 7.4 Weather menu actions

`WeatherTab.menuActions` is the custom action list used by `DashPageActions`:

- Refresh Weather: calls `WeatherService.forceRefresh()`.
- Settings: closes the dashboard and opens `weather` settings.
  (`WeatherTab.qml:22-37`)

`DashPageActions` builds a `CcMenu` from `tabItem.menuActions`, so any tab can
add custom actions without changing the page actions component
(`DashPageActions.qml:56-60`, `73`).

### 7.5 Island weather

`WeatherExpanded` is a `DashTabFace` that wraps `WeatherTab`:

```qml
DashTabFace {
    activityId: "weather"
    tabComponent: Component {
        WeatherTab {
            live: root.live
            editMode: root.editMode
            transientSurfaceTracker: root.controller.transientSurfaces
        }
    }
}
```

(`Modules/DIsland/Activities/WeatherExpanded.qml:6-16`)

`DashTabFace` supplies:

- a sheet handle that enters edit mode (`154-160`),
- `DashPageActions` in the header (`162-179`),
- a `DashOptionsSheet` for tab options (`251-256`),
- `DGridEditChrome` for panel resize (`224-243`),
- F2/Ctrl+E and Escape handling (`117-139`),
- `panelResizer` that commits `panelColumns`/`panelRows` through
  `DashRegistry.setOptions` (`40-66`).

Island weather is opened through `IslandController.requestWeather`, which checks
`SettingsData.weatherEnabled` and toggles collapse if already expanded
(`Modules/DIsland/IslandController.qml:798-802`, `725`). The compact face is a
`DestinationCompact` with a weather icon and label
(`Modules/DIsland/DIslandSurface.qml:738-756`).

The island uses the same edit semantics as the dashboard: immediate persistence,
no snapshot/undo.

### 7.6 Weather settings page

`WeatherSettingsTab` is a normal settings page:

- General: enable weather, imperial units, wind speed unit.
- Location: auto location, location search, custom latitude/longitude.
  Coordinates are validated and saved with `SettingsData.setWeatherLocation`.
- Display: lock screen weather toggle, nav to Dashboard, nav to Bar widgets.
  (`Modules/Settings/WeatherSettingsTab.qml:7-155`)

Weather settings are global. The dashboard weather tab's per-widget options are
not here; they live in `dashOptions.weather.widgets`. The bar weather widget's
only option is the nav card to this page.

### 7.7 Bar weather widget

`Modules/DBar/Widgets/Weather.qml`:

- visible only when `SettingsData.weatherEnabled`,
- adds and removes a weather service reference,
- renders an icon and temperature, horizontal or vertical,
- has no `clicked` handler, no popout target, and no context menu.

This is an intentional split: the bar widget is a status indicator, not a menu.
If a replication needs a weather popout, it must be designed as a new surface,
not inferred from the bar widget.

### 7.8 Weather data summary

- Global enable and units: `SettingsData.weatherEnabled`, `useFahrenheit`,
  `windSpeedUnit`, `useAutoLocation`, `weatherLocation`,
  `weatherCoordinates`.
- Dashboard card options: `dashOptions.weather` (`city`, `readings`, `tone`).
- Weather tab widget layout and per-widget options:
  `dashOptions.weather.widgets`.
- Weather tab panel size: `dashOptions.weather.panelColumns`, `panelRows`.
- Existing widget definitions carry default sizes, min/max, and option lists
  directly in `WeatherTab.qml`.

---

## 8. Launcher

Primary files:

- `Modals/DLauncherV2/DLauncherV2ModalHost.qml`
- `Modals/DLauncherV2/LauncherContent.qml`
- `Modals/DLauncherV2/SpotlightLauncherContent.qml`
- `Modals/DLauncherV2/Controller.qml`
- `Modals/DLauncherV2/ResultsList.qml`
- `Modals/DLauncherV2/LauncherContextMenu.qml`
- `Modals/DLauncherV2/ActionPanel.qml`
- `Modals/DLauncherV2/AppEditView.qml`
- `Modules/Settings/LauncherTab.qml`
- `Common/SessionData.qml`

### 8.1 Hosts

The launcher content is reused by:

- the full floating launcher,
- the spotlight launcher,
- the island launcher,
- possibly other modal hosts.

`DLauncherV2ModalHost` creates either `LauncherContent` or
`SpotlightLauncherContent` behind a loader
(`DLauncherV2ModalHost.qml:856-910`). `LauncherExpanded` uses
`SpotlightLauncherContent` directly inside the island and supplies a
`controllerOverride` from `IslandController`
(`Modules/DIsland/Activities/LauncherExpanded.qml:67-81`).

This means the menus are host-independent. Context menus and the action panel
only need a controller and a parent handler.

### 8.2 Controller and result model

`Controller` is a plain QML controller singleton-like object with:

- `searchQuery`, `searchMode`, `sections`, `flatModel`, `selectedFlatIndex`,
  `selectedItem`.
- plugin and file search state.
- view mode, grid columns, collapsed sections.
- dmenu state.
- history and selection.
  (`Modals/DLauncherV2/Controller.qml:17-66`, `254-552`)

Item transforms produce result types such as:

- `app`
- `plugin`
- `plugin_browse`
- `setting`
- `file`
- `clipboard`
- `dmenu`

Selection is maintained by `selectNext`, `selectPrevious`, `selectLeft`,
`selectRight`, `selectPageDown`, `selectPageUp`, `selectIndex`, and
`selectNextSection` (`Controller.qml:2519-2610`). `updateSelectedItem()` sets
`selectedItem`.

Execution is centralized:

- `executeSelected` handles search pending and dmenu multi-select.
- `executeItem` branches by item type.
- `executeAction` handles action IDs such as `launch`, `open`, `open_folder`,
  `open_with`, `copy_path`, `open_terminal`, `copy`, `execute`,
  `clipboard_copy`, `clipboard_paste`, `launch_dgpu`,
  `toggle_all_visibility`, and desktop-entry action data.
  (`Controller.qml:2648-2805`)

This means the context menu and action panel do not launch anything directly.
They call `controller.executeAction(item, action)` or the specific helper.

### 8.3 Results list and right-click

`ResultsList` builds a visual row model from sections and view modes:

- list items,
- grid rows,
- tile rows,
- section headers and spacers.
  (`Modals/DLauncherV2/ResultsList.qml:20-130`)

It emits:

```qml
signal itemRightClicked(int index, var item, real mouseX, real mouseY)
```

(`ResultsList.qml:36`)

List, grid, and tile delegates all emit the same signal
(`ResultsList.qml:397`, `459`, `481`). The host maps scene coordinates to local
coordinates and calls the context menu.

`LauncherContent` wires it:

```qml
onItemRightClicked: (index, item, sceneX, sceneY) => {
    if (item && contextMenu.hasContextMenuActions(item)) {
        var localPos = root.mapFromItem(null, sceneX, sceneY);
        root.showContextMenu(item, localPos.x, localPos.y, false);
    }
}
```

(`LauncherContent.qml:682-687`)

Spotlight does the same with its own root mapping
(`SpotlightLauncherContent.qml:448-450`).

Keyboard access is Menu or F10 on the selected item, using
`resultsList.getSelectedItemPosition()` as the anchor
(`LauncherContent.qml:263-270`, `SpotlightLauncherContent.qml:263-271`).

### 8.4 Launcher context menu

`LauncherContextMenu` computes its items from the selected item:

- Plugin item: plugin-provided `getContextMenuActions`, mapped to menu items
  (`LauncherContextMenu.qml:36-42`).
- Non-app item with actions: those actions (`44-50`).
- App:
  - Pin or unpin from dock, when a dock is available for the screen
    (`53-59`).
  - Hide App (`60-66`).
  - Edit App (`67-72`).
  - Desktop-entry actions after a separator (`74-85`).
  - Launch on dGPU when available (`89-95`).
  - Launch (`96-101`).
  - Uninstall for Flatpak apps, destructive (`102-111`).
  (`35-113`)

`hasContextMenuActions` decides whether a right-click does anything:

- all apps have a menu,
- plugins only if they return actions,
- other types only if they have actions.
  (`124-137`)

Opening:

- `show(x, y, item, fromKeyboard)` stores the item.
- It resolves the screen from the modal, parent handler, or search field.
- It converts local coordinates to screen-relative coordinates.
- It picks the dock ID relevant to that screen.
- It disables the parent handler while the menu is open.
- It calls `DContextMenu.open(screen, x + spacing, y + spacing, fromKeyboard)`.
  (`149-165`)

Closing:

- re-enables the parent handler and returns focus to the search field
  (`115-122`).

Actions:

- `executePluginAction` calls the plugin action, optionally closes the launcher,
  otherwise refreshes results, and hides the menu (`167-177`).
- `executeLauncherAction` delegates to `controller.executeAction` (`179-184`).
- `togglePin` writes dock pins through `SessionData` (`186-194`).
- `hideCurrentApp` writes `SessionData.hideApp` and refreshes results
  (`196-202`).
- `editCurrentApp` emits `editAppRequested` for the host to open `AppEditView`
  (`204-209`).
- `uninstallCurrentApp` hides the menu, closes the launcher, and requests the
  Flatpak uninstall (`211-220`).
- `launchApp`, `launchWithNvidia`, and `executeDesktopAction` launch and record
  usage (`222-255`).

### 8.5 Action panel

The action panel is not a popup. It is a horizontal strip inside the launcher
that expands under the results.

`ActionPanel` builds an action list:

- start with `selectedItem.primaryAction`,
- add plugin context actions as `plugin_action`,
- add app actions, file actions, clipboard actions,
- add "Launch on dGPU" for apps when available.
  (`ActionPanel.qml:46-104`)

`hasActions` decides whether Tab does anything:

- apps always have actions (except core apps),
- plugins only with context actions,
- plugin browse, files, and clipboard only with actions,
- other types when more than one action.
  (`106-119`)

The panel:

- animates its height,
- renders action buttons in a horizontal flickable,
- highlights the hovered or keyboard-selected action,
- shows a Tab key hint,
- scrolls the selected action into view,
  (`121-202`, `225-240`).

Keyboard behavior:

- Tab or Shift+Tab shows or cycles actions.
- Enter executes the selected action if the index is greater than zero,
  otherwise executes the item.
  (`LauncherContent.qml:238-260`,
  `SpotlightLauncherContent.qml:237-260`)

The index-greater-than-zero rule means the primary action is index 0, and Enter
without explicitly selecting an action still executes the item normally. This is
a subtle UX detail worth copying.

### 8.6 Edit App view

`AppEditView` is a full-content replacement, not a dialog:

- Loads an existing override from `SessionData.getAppOverride(editAppId)`.
- Fields: name, icon, description/comment, environment variables, extra
  arguments.
- Save builds an override object and calls `SessionData.setAppOverride`.
- Reset calls `SessionData.clearAppOverride`.
- Escape closes; Ctrl+Enter or Ctrl+S saves.
  (`Modals/DLauncherV2/AppEditView.qml:16-60`, `250-274`)

`LauncherContent` and `SpotlightLauncherContent` manage edit mode:

- `editMode`, `editingApp`, `editAppId`.
- `openEditMode(app)` records the app ID and enables edit mode.
- `closeEditMode()` clears the state and focuses the search field.
- The results/action panel are hidden while editing.
- `AppEditView` is loaded into the content area and `loadOverride()` is called
  after load.
  (`LauncherContent.qml:27-58`, `716-732`;
  `SpotlightLauncherContent.qml:30-101`, `464-483`)

The identity used for an override is `app.id || app.execString || app.exec`
(`LauncherContent.qml:48-49`, `SpotlightLauncherContent.qml:93-94`).

### 8.7 Hidden apps and overrides in settings

`LauncherTab` provides the persistent management view:

- Hidden apps card:
  - model built from `SessionData.hiddenApps` joined with
    `AppSearchService.applications`,
  - instruction text: right-click an app and choose Hide App,
  - Show button calls `SessionData.showApp`.
- App customizations card:
  - model built from `SessionData.appOverrides`,
  - Reset button calls `SessionData.clearAppOverride`,
  - instruction text: right-click an app and choose Edit App.
  (`Modules/Settings/LauncherTab.qml:421-574`)

Persistence:

- `hideApp`, `showApp`, `isAppHidden`, `setAppOverride`, `getAppOverride`,
  `clearAppOverride` all call `saveSettings()` in `SessionData`
  (`Common/SessionData.qml:1614-1662`).
- The session store schema declares `hiddenApps` and `appOverrides`
  (`Common/settings/SessionSpec.js:195-200`).

### 8.8 Launcher settings that affect menus

Other launcher settings are normal settings page rows, not context menus:

- default action and style (`launcherStyle`),
- size, grid columns, footer, source badges, background darkening,
- search options, enter action, history, file inclusion,
- keybinds,
- built-in apps and plugins visibility,
- spotlight bar,
- launching prefix,
- recent apps,
- advanced layer and unload behavior.
  (`Modules/Settings/LauncherTab.qml:89-969`)

These are persisted in `SettingsData`, while hidden apps and app overrides are
session data.

### 8.9 End-to-end flows

**Right-click an app.** Results list emits `itemRightClicked`. Host opens
`LauncherContextMenu`. Menu items are generated from the app type. Choosing Hide
App writes `SessionData.hiddenApps`; choosing Edit App opens `AppEditView`.

**Use the action strip.** Press Tab. `ActionPanel` expands and shows the primary
action plus secondary actions. Tab/Shift+Tab cycles. Enter executes the selected
action if one is selected, otherwise launches the item.

**Edit app metadata.** Right-click, Edit App. The results UI is replaced by the
form. Save writes an override. The launcher refreshes and the next search uses
the override.

**Manage hidden apps.** Settings, Launcher, Hidden apps. Each row shows the app
and a Show button. This is the same data the context menu writes.

**Keyboard-only context menu.** Select a result, press Menu or F10, the menu
opens at the selected item position with `fromKeyboard = true`, then arrow keys
and Enter operate it.

---

## 9. Cross-cutting patterns worth copying

### 9.1 Route IDs and aliases

Every settings page has a stable ID, display text, optional aliases, parent, and
optional hub header. `SettingsTabs.resolvePage` normalizes aliases and resolves
single-leaf hubs directly. This makes deep links stable even if the sidebar
structure changes.

Apply this to a GTK settings GUI as a `PageRegistry` with:

- canonical ID,
- aliases,
- parent ID,
- title and icon,
- page factory,
- optional deep-link parameters.

### 9.2 Spec-driven options

Do not hard-code a form per widget. Use descriptors:

```text
OptionSpec {
  key, label, kind, default, choices, min, max, step, unit,
  description, scope
}
```

Scopes matter:

- `cardOnly`: only for card instances.
- `settingsOnly`: only in full settings, not in quick sheets.
- `tabScope`: exclude card-only options when editing a tab.

The same descriptor list can render a compact sheet, a full settings card, and
a searchable settings index.

### 9.3 Two-tier persistence

Store only non-default overrides when possible. Resolve on read. The dashboard
does this aggressively: `DashRegistry.setOptions` deletes a key when it equals
the default and deletes the whole entry when empty
(`DashRegistry.qml:550-575`). This keeps the config small and makes defaults
upgradeable.

Bar widgets use a simpler variant: the entry stores options directly, and the
resolver falls back to type defaults. Both are valid; the important thing is a
single resolver, not conditionals scattered through the UI.

### 9.4 Edit sessions

There are two valid models in the tree:

- **Transactional:** control center. Snapshot, undo stack, batched changes,
  discard rollback, finish commit.
- **Immediate:** dashboard and bar widgets. Every edit writes through, and reset
  or clear are explicit actions.

Pick one per surface. Do not mix them silently. Control center uses transactional
because the grid can be rearranged destructively and offers undo. Dashboard uses
immediate because every widget renders from the live config and there is no
separate "apply".

If implementing a generic edit session, the control center is the richer model:

```text
begin:
  snapshot = serialize(model)
  undoBase = snapshot
  undo = []
change:
  if not batching and serialize(model) != undoBase:
      undo.push(undoBase)
      undoBase = serialize(model)
batch:
  suppress intermediate pushes
  push one entry for the whole batch
undo:
  apply(undo.pop())
cancel:
  apply(snapshot)
finish:
  clear snapshot and undo
```

### 9.5 Generic edit grid

The reusable part is not the tile widgets. It is:

- a pure layout packer,
- item size constraints,
- a slot wrapper that owns resize gestures,
- a chrome overlay that owns remove/options/resize buttons,
- a commit callback that replaces the whole model,
- a preview path that does not mutate the model until drop or resize end.

For a GTK version, use a `Gtk.Fixed` or custom layout inside a `Gtk.ScrolledWindow`
and `Gtk.GestureDrag` for move/resize. Keep the model as the source of truth and
only render a preview rect during drag.

### 9.6 Context menu construction

Always build menu entries from the current selection type:

```text
if plugin: plugin actions
if app: pin/hide/edit/desktop actions/launch/uninstall
if file: open/open folder/copy path/terminal
if clipboard: copy/paste
```

This avoids a giant "enable only the relevant items" menu. The launcher does
this well. The control center does a simpler version with a fixed edit menu.

### 9.7 Deep links into live surfaces

Some settings are better edited in context. The dashboard settings page does not
duplicate the grid editor. It calls `openDankDashEditor(tabId, screen)` and
opens the actual popout in edit mode. A GTK app can reproduce this by exposing a
"Edit in place" command that opens the relevant surface and sets its edit state.

### 9.8 Destructive action handling

Three patterns appear:

- control center: destructive clear is undoable, reset is batched into undo,
  explicit Discard rolls back the whole session.
- dashboard: reset and clear use a two-step inline confirm.
- launcher: uninstall and hide are single menu actions; uninstall closes the
  launcher and delegates to a service.

There is no single correct policy. The reusable idea is that destructive actions
must either be undoable or confirmed, and the choice should be visible in the
menu item (`destructive` styling, arming state, or undo affordance).

### 9.9 Focus and transient surfaces

The tree is careful about focus:

- menus restore focus to their anchor,
- launcher menus restore focus to the search field,
- detail pages return focus to the invoking item,
- settings pages remember the focused control when navigating away,
- transient surface trackers close nested menus/dropdowns.

A GTK replication should use explicit focus restoration, not rely on the window
manager to guess. Store the invoking widget or a stable control ID before
opening a menu or page.

### 9.10 Keyboard parity

Every menu system has keyboard paths:

- context menu: Menu/F10, arrow keys, Enter.
- action panel: Tab/Shift+Tab, Enter.
- edit mode: F2/Ctrl+E, Escape, Tab traversal.
- reorder: drag handle plus Ctrl+arrow.
- settings navigation: arrow keys, Backspace/Alt+Left, Escape.
- dialogs: Escape, Tab/Backtab, Enter.

When replicating, treat keyboard operation as part of the menu contract, not an
accessibility afterthought.

---

## 10. Replication blueprint for nix-settings-gui

This section maps the observed Dux patterns onto a GTK4/libadwaita settings GUI.
It is not a claim that the current repo needs these exact structures; it is a
practical translation of the behavior above.

### 10.1 Core data types

```rust
struct PageSpec {
    id: String,
    aliases: Vec<String>,
    title: String,
    icon: Option<String>,
    parent: Option<String>,
    kind: PageKind,          // Normal, Hub, Plugin
    settings_tab: Option<String>,
}

struct OptionSpec {
    key: String,
    label: String,
    description: String,
    kind: OptionKind,        // Toggle, Choice, Number, Text, Color, Action
    default: Value,
    choices: Vec<Choice>,
    min: Option<f64>,
    max: Option<f64>,
    step: Option<f64>,
    scope: OptionScope,      // Card, SettingsOnly, Both
}

struct MenuItemSpec {
    id: String,
    label: String,
    icon: Option<String>,
    enabled: bool,
    destructive: bool,
    action: Action,
}
```

### 10.2 UI mapping

- Settings window: `adw::ApplicationWindow` plus `adw::NavigationSplitView`.
  Sidebar `gtk::ListBox` or `adw::Sidebar`. Content `adw::NavigationView` or
  `gtk::Stack`. Keep a `Vec<String>` history in the app state.
- Context menu: `gtk::Popover` attached to the invoking widget. Build children
  from `MenuItemSpec`. Restore focus to the invoking widget on close.
- Large popout: a separate `gtk::Window` or an `adw::Overlay` anchored panel.
- Bottom sheet: `adw::BottomSheet` if available in the target libadwaita
  version, otherwise a `gtk::Revealer` overlay inside the popout.
- Dialog: `adw::Dialog` or `gtk::Dialog` for forms and destructive confirms.
- Grid editor: custom `gtk::Fixed` inside a `gtk::ScrolledWindow` with a model
  adapter, or a `gtk::GridView` if all items share a cell size.
- Reorder: `gtk::DragSource` + `gtk::DropTarget`, with model index updates, or
  explicit up/down move buttons for keyboard parity.

### 10.3 State separation

Keep these independent:

1. **Persistent config model.** Serialized to disk.
2. **Runtime session state.** Selected tab, selected bar, selected widget index,
   edit mode, open page history.
3. **Transient UI state.** Open menu, open sheet, drag preview, hover, focus.
4. **Derived view model.** Resolved options with defaults applied.

Do not store resolved values back into the config unless the user changes them.
Do not store selected indices in the persistent config.

### 10.4 Menu construction

Use a trait or enum for menu actions:

```rust
enum MenuAction {
    Navigate(String),
    EditInPlace { surface: String, target: String },
    SetOption { target: String, key: String, value: Value },
    Reorder { from: usize, to: usize },
    Remove { target: String },
    Launch { app_id: String },
    HideApp { app_id: String },
    EditApp { app_id: String },
    Custom(Box<dyn Fn()>),
}
```

Build menus from the selected object, not from a static tree. This mirrors the
launcher context menu and the dashboard options sheet.

### 10.5 Edit sessions

For any collection that supports undo or cancel, implement:

```text
EditSession<T> {
    original: T,
    current: T,
    undo: Vec<T>,
    redo: Vec<T>,
    batching: bool,
}
```

Commit writes `current` to the config. Cancel writes `original` back. Undo
restores from `undo`. Batch changes collapse into one undo entry. Gate undo while
a drag or resize gesture is active.

For immediate editors, skip the session and write through, but still provide
explicit Reset and Clear commands with confirmation or undo.

### 10.6 Searchable settings

The Dux settings cards register a `settingKey` and tags, and
`SettingsSearchService` uses them for navigation and highlighting. A GTK version
can keep a registry:

```text
SearchEntry {
    page_id,
    section_id,
    option_key,
    title,
    description,
    tags,
    keywords,
}
```

Search results should navigate to the page, not just filter a list. The Dux
search service returns a page and highlights the target section.

### 10.7 Deep links and aliases

Implement `resolve_page(raw: &str) -> Option<PageId>`:

- exact ID,
- alias,
- plugin prefix,
- single-leaf hub resolution,
- fallback to parent or default.

This is what allows command-line IPC, context menus, and settings search to all
use the same navigation API.

### 10.8 Option page adapters

For repeated editors such as bar widgets, define an adapter trait:

```rust
trait OptionStore {
    fn get(&self, key: &str) -> Value;
    fn set(&mut self, key: &str, value: Value);
    fn is_default(&self, keys: &[&str]) -> bool;
    fn reset(&mut self, keys: &[&str]);
}
```

Then one widget settings page can target a bar entry, a dock entry, or a
standalone instance. This is exactly what `BarWidgetTab` does with its
`store`/`appStore` objects.

### 10.9 What not to copy literally

- Do not copy QML's `Qt.callLater` deferrals unless the GTK equivalent has the
  same destruction-during-signal hazard. They are there for QML view lifecycle
  reasons.
- Do not copy layer-shell namespaces as a UI concept. They are implementation
  details of Quickshell surfaces.
- Do not copy the broken submodule API names if a simpler GTK widget exists.
  The reusable concept is the contract, not the QML class names.
- Do not assume dashboard and control center share edit semantics. They do not.
  Control center rolls back; dashboard writes through.

---

## 11. File and symbol index

### Shared

- `Common/SettingsData.qml`: persistent settings properties and helpers.
- `Common/settings/SettingsSpec.js`: defaults and change hooks.
- `Common/SettingsTabs.qml`: page route table and resolver.
- `Common/SettingsUiState.qml`: selected bar/dock/widget context.
- `Common/BarWidgetCatalog.qml`: bar widget catalog and options-file mapping.
- `Common/settings/BarWidgetDefaults.js`: widget type defaults and resolver.
- `Common/SessionData.qml`: hidden apps, app overrides, dock pins, launcher
  history.
- `Common/settings/SessionSpec.js`: session schema.
- `Modals/Settings/SettingsModal.qml`: settings window, page history, deep links.
- `Modals/Settings/SettingsContent.qml`: page registry, `StackView`, property
  injection.
- `Modules/Settings/Widgets/SettingsPage.qml`: page scroll container.
- `Modules/Settings/Widgets/SettingsCard.qml`: titled card and search
  registration.
- `Modules/Settings/Widgets/SettingsRow.qml`: base row and reset behavior.
- `Modules/Settings/Widgets/SettingsToggleRow.qml`: toggle row.
- `Modules/Settings/Widgets/SettingsReorderList.qml`: reorder list adapter.
- `Modules/Settings/Widgets/SettingsReorderRow.qml`: drag handle and keyboard
  move.
- `Modules/Settings/Widgets/SettingsReorderGroup.qml`: cross-list group.
- `Modules/Settings/Widgets/WidgetSelectionPopup.qml`: add-widget picker.
- `Modules/Settings/WidgetPickerWindow.qml`: picker window implementation.

### Control center

- `Modules/ControlCenter/ControlCenterPopout.qml`: popout and settings handoff.
- `Modules/ControlCenter/ControlCenterContent.qml`: edit state, grid host,
  detail page, widget sheet, config overlay.
- `Modules/ControlCenter/Components/CcEditActions.qml`: edit strip and more menu.
- `Modules/ControlCenter/Components/CcWidgetSheet.qml`: add-widget sheet.
- `Modules/ControlCenter/Components/WidgetConfigOverlay.qml`: per-tile options.
- `Modules/ControlCenter/Components/CcTileGrid.qml`: grid adapter.
- `Modules/ControlCenter/Components/CcTileSlot.qml`: slot resize and chrome.
- `Modules/ControlCenter/Components/CcDetailPage.qml`: detail pages.
- `Modules/ControlCenter/Components/QuickTilesGroup.qml`: grouping.
- `Modules/ControlCenter/Models/WidgetModel.qml`: catalog and component map.
- `Modules/ControlCenter/utils/widgets.js`: control center model operations.
- `Modules/ControlCenter/utils/sections.js`: instance-scoped detail sections.
- `Modules/ControlCenter/CcMetrics.qml`: sizes and dialog metrics.
- `Modules/ControlCenter/Widgets/CcMenu.qml`: context menu wrapper.
- `Modules/ControlCenter/Widgets/CcSheetDialog.qml`: in-popout dialog.
- `Modules/ControlCenter/Widgets/CcSettingsButton.qml`: detail-to-settings
  handoff.

### Bar

- `Modules/DBar/WidgetModel.js`: normalize and component map.
- `Modules/SurfaceWidgets/SurfaceWidgetHost.qml`: widget loading and property
  injection.
- `Modules/SurfaceWidgets/BarPill.qml`: click/right-click/popout anchor.
- `Services/BarWidgetService.qml`: widget registry and popout trigger.
- `Modules/Settings/WidgetsTab.qml`: bar widgets hub.
- `Modules/Settings/WidgetsTabSection.qml`: section list.
- `Modules/Settings/BarWidgetTab.qml`: per-widget settings adapter.
- `Modules/Settings/BarWidgetOptions/*`: option pages.
- `Modules/DBar/Widgets/Weather.qml`: display-only weather bar widget.

### Dashboard

- `Modules/DDash/DDashPopout.qml`: popout, tabs, edit mode.
- `Modules/DDash/DashTabHost.qml`: tab loading and injection.
- `Modules/DDash/DashRegistry.qml`: schema, options, tab resolution.
- `Modules/DDash/OverviewTab.qml`: overview grid host.
- `Modules/DDash/Overview/DashCardGrid.qml`: card grid adapter.
- `Modules/DDash/Overview/DashCardSlot.qml`: card slot and chrome.
- `Modules/DDash/Overview/DashEditChrome.qml`: card chrome.
- `Modules/DDash/DashPageActions.qml`: page edit actions.
- `Modules/DDash/Overview/DashEditControls.qml`: edit button strip.
- `Modules/DDash/Overview/DashOptionsSheet.qml`: options sheet.
- `Modules/DDash/DashWidgetGrid.qml`: tab-local widget grid.
- `Modules/DDash/utils/cards.js`: card layout persistence.
- `Modules/DDash/utils/options.js`: option value resolution.
- `Modules/DDash/utils/widgets.js`: widget layout resolution.
- `Modules/Settings/DDashTab.qml`: dashboard settings page.

### Weather

- `Modules/DDash/WeatherTab.qml`: weather tab layout and menu actions.
- `Modules/DDash/Overview/WeatherOverviewCard.qml`: dashboard card.
- `Modules/DDash/Weather/*`: weather widget visuals.
- `Modules/DIsland/Activities/WeatherExpanded.qml`: island weather face.
- `Modules/DIsland/Activities/DashTabFace.qml`: island tab edit chrome.
- `Modules/Settings/WeatherSettingsTab.qml`: global weather settings.
- `Modules/DBar/Widgets/Weather.qml`: bar indicator.

### Launcher

- `Modals/DLauncherV2/Controller.qml`: state and execution.
- `Modals/DLauncherV2/ResultsList.qml`: result model and right-click signal.
- `Modals/DLauncherV2/LauncherContextMenu.qml`: context menu and actions.
- `Modals/DLauncherV2/ActionPanel.qml`: action strip.
- `Modals/DLauncherV2/AppEditView.qml`: app override form.
- `Modals/DLauncherV2/LauncherContent.qml`: full launcher host.
- `Modals/DLauncherV2/SpotlightLauncherContent.qml`: spotlight host.
- `Modules/Settings/LauncherTab.qml`: launcher settings and hidden apps.
- `Common/SessionData.qml`: hidden apps and app overrides.

---

## 12. Gaps, limits, and gotchas

1. **The common QML library is missing from this checkout.** The exact
   implementation of `DContextMenu`, `DPopout`, `DEditableGrid`,
   `DGridEditChrome`, `DReorderList`, `DPanelResizer`, and `DBottomSheet` could
   not be read. Their contracts are inferred from call sites. If exact behavior
   matters, obtain the `dux-qml-common` submodule at the matching commit.

2. **Control center and dashboard do not share edit semantics.** Control center
   has snapshot, undo, discard, and batched reset. Dashboard writes through
   immediately and uses two-step confirmation for destructive actions. A
   replication must choose deliberately.

3. **Bar widget "edit menus" are in the settings window, not on the bar.**
   Right-click on a bar widget is a widget-specific action. There is no generic
   right-click-to-configure gesture. The weather bar widget has no menu at all.

4. **Duplicate widget identities use indexes, not type IDs.** Bar widget lists
   use `(section, index)` and control center uses the saved array index. A
   replication should use stable instance IDs where possible, but must still
   support duplicates.

5. **Some options are scope-sensitive.** Dashboard panel width/height are
   settings-only, card options are card-only, and card options disappear in tab
   scope. A generic option sheet that ignores scope will show invalid controls.

6. **Some menu entries are closed-then-opened surfaces.** Control center to
   settings, weather tab to settings, and dashboard edit-in-place all close one
   surface before opening another. Relying on the target surface to open while
   the source is still animating can produce focus races.

7. **Settings page state can be context-dependent.** `BarWidgetTab` depends on
   singleton fields (`SettingsUiState.selectedBarId`, `selectedWidgetSection`,
   `selectedWidgetIndex`, and dock equivalents). Deep-linking directly to the
   page without setting that context produces an empty page.

8. **The weather tab's default widget set includes disabled widgets.**
   `DashWidgetGrid.addable` filters only on presence, not on `enabled: false`,
   while default resolution excludes disabled definitions. If replicating this,
   decide explicitly whether disabled definitions are addable.

9. **The dashboard weather tab is reused in the island.** Any change to
   `WeatherTab` or `DashWidgetGrid` affects both dashboard and island. A GTK
   replication should factor the weather content from its host chrome for the
   same reason.

10. **Launcher app overrides are session-scoped, not settings-scoped.** They are
    stored in `SessionData.appOverrides`, not `SettingsData`. This matters for
    backup, export, and reset behavior.

11. **The launcher action strip uses a special Enter rule.** Enter executes the
    selected action only when its index is greater than zero; otherwise it
    executes the result. Copying a simpler "always execute selected action"
    rule will change the common path.

12. **Search is part of navigation.** Settings cards and rows register setting
    keys and tags, and launcher results are built from multiple sources with
    section collapsing and view modes. A menu replication that ignores search
    will feel structurally different even if the pages look the same.
