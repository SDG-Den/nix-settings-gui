# general

ordering is currently not done that well, probably because group sort, grouping, submodules, ordering and sorting all interfere, this needs to be resolved with default ordering + user-configurable overrides. primarily at a group level. 

systems menu setting also needs a special key "all" to make a file always available to all systems as if it was system-specific. 

widgets should be moved to be left-alligned below their text.
widgets and text should both be full-width when needed (text input modules)
imported from text needs to be below the widget body
submodules have one set of text above, and one imported from at the bottom
submodules need to be nestable, each submodule indents a bit more and has its own header and imported-from footer
new horizontal submodule (non-nestable) that lists multiple options side-by-side with just names above, overflows to second row if it doesn't fit on one row to result in an equal grid. 

new submodule "table", with submodules to generate a new attrset with the entries (table-attrset-row, makes {...; ...;}) or a new list (table-list-row, makes [ "" "" ]), this should use auto-detection for true/false and numbers for the cells so cells are not just plain control. 

"add" button for snippets currently results in confusion, users type "Add" into the button text in their nix config, resulting in double Add. 

hidden/advanced options need to still adhere to this inside a submodule



# ojuny-template/desktop specific

see ~/repos/ojuny-desktop and ~/repos/ojuny-template

new special widgets for:

- keybinds

- monitors

- grid-configuration (for dashboard and control center)

- theme selection

- menu configuration (might be satisfied with the table submodule)

- plugins configuration 

- bar widgets configuration (maybe this doesnt need anything special?)

these will all have to be designed specifically based on the shape of each config thing. application for this is in ojuny-template, not in ojuny-desktop directly. 

