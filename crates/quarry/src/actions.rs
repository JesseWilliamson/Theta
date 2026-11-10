//! Every user-invokable action, in one place, so the command palette, the
//! default keymap, the macOS menu and view code all refer to the same types.

use gpui_kit::actions;

actions!(
    quarry,
    [
        // Workspace
        NewConsole,
        NewConnection,
        ConnectTo,
        Disconnect,
        RefreshSchema,
        ToggleExplorer,
        ToggleBottomPanel,
        CloseTab,
        NextTab,
        PreviousTab,
        FocusExplorer,
        // Command surfaces
        OpenCommandPalette,
        OpenTablePicker,
        OpenConnectionPicker,
        SelectTheme,
        ToggleKeymap,
        ImportZedTheme,
        OpenSettingsFile,
        ZoomIn,
        ZoomOut,
        ZoomReset,
        Quit,
        // Seams: the editor/results crates implement what these do. The shell
        // registers the handlers and re-emits them as `WorkspaceEvent`s.
        RunStatement,
        RunAll,
        CancelQuery,
        Explain,
        ExplainAnalyze,
        FormatDocument,
        // Explorer (key context "Explorer")
        ExplorerNext,
        ExplorerPrevious,
        ExplorerExpand,
        ExplorerCollapse,
        ExplorerActivate,
        ExplorerFilter,
    ]
);
