#![allow(clippy::wildcard_imports)]

mod scratchpad_handler;
// Make public to the rest of the crate without exposing other internal
// details of the scratchpad handling code
pub use scratchpad_handler::{Direction, ReleaseScratchPadOption};

use super::*;
use crate::display_action::DisplayAction;
use crate::display_servers::DisplayServer;
use crate::layouts::Layout;
use crate::models::{TagId, WindowState};
use crate::state::State;
use crate::utils::helpers;
use crate::utils::helpers::relative_find;
use crate::{config::Config, models::FocusBehaviour};

impl<C: Config, SERVER: DisplayServer> Manager<C, SERVER> {
    /* When adding a command
     * please update src/utils/command_pipe and leftwm/src/command if:
     * - a command is introduced or renamed
     * please also update src/bin/leftwm-check if any of the following apply after your update:
     * - a command now requires a value
     * - a command no longer requires a value
     * - a new command is introduced that requires a value
     *  */
    /// Processes a command and invokes the associated function.
    pub fn command_handler(&mut self, command: &Command) -> bool {
        process_internal(self, command).unwrap_or(false)
    }
}

macro_rules! move_focus_common_vars {
    ($func:ident ($state:expr $(, $arg:expr )* $(,)? )) => {{
        let handle = $state.focus_manager.window(&$state.windows)?.handle;
        let tag_id = $state.focus_manager.tag(0)?;
        let tag = $state.tags.get(tag_id)?;
        let layout = Some(tag.layout);

        let for_active_workspace =
            |x: &Window| -> bool { x.tag == Some(tag_id) && x.is_managed() };

        let to_reorder = helpers::vec_extract(&mut $state.windows, for_active_workspace);
        $func($state, handle, layout, to_reorder, $($arg),*)
    }};
}

fn process_internal<C: Config, SERVER: DisplayServer>(
    manager: &mut Manager<C, SERVER>,
    command: &Command,
) -> Option<bool> {
    let state = &mut manager.state;
    match command {
        Command::ToggleScratchPad(name) => scratchpad_handler::toggle_scratchpad(manager, name),
        Command::AttachScratchPad { window, scratchpad } => {
            scratchpad_handler::attach_scratchpad(*window, scratchpad, manager)
        }
        Command::ReleaseScratchPad { window, tag } => {
            scratchpad_handler::release_scratchpad(window.clone(), *tag, manager)
        }

        Command::NextScratchPadWindow { scratchpad } => {
            scratchpad_handler::cycle_scratchpad_window(manager, scratchpad, Direction::Forward)
        }
        Command::PrevScratchPadWindow { scratchpad } => {
            scratchpad_handler::cycle_scratchpad_window(manager, scratchpad, Direction::Backward)
        }

        Command::ToggleFullScreen => toggle_state(state, WindowState::Fullscreen),
        Command::ToggleSticky => toggle_state(state, WindowState::Sticky),

        Command::SendWindowToTag { window, tag } => move_to_tag(*window, *tag, manager),
        Command::MoveWindowToNextTag { follow } => move_to_tag_relative(manager, *follow, 1),
        Command::MoveWindowToPreviousTag { follow } => move_to_tag_relative(manager, *follow, -1),
        Command::MoveWindowToLastWorkspace => move_to_last_workspace(state),
        Command::MoveWindowToNextWorkspace => move_window_to_workspace_change(manager, 1),
        Command::MoveWindowToPreviousWorkspace => move_window_to_workspace_change(manager, -1),
        Command::MoveWindowUp => move_focus_common_vars!(move_window_change(state, -1)),
        Command::MoveWindowDown => move_focus_common_vars!(move_window_change(state, 1)),
        Command::MoveWindowTop { swap } => move_focus_common_vars!(move_window_top(state, *swap)),

        Command::GoToTag { tag, swap } => goto_tag(state, *tag, *swap),
        Command::ReturnToLastTag => return_to_last_tag(state),

        Command::CloseWindow => close_window(state),
        Command::SwapScreens => swap_tags(state),
        Command::NextLayout => next_layout(state),
        Command::PreviousLayout => previous_layout(state),

        Command::SetLayout(layout) => set_layout(*layout, state),

        Command::FloatingToTile => floating_to_tile(state),
        Command::TileToFloating => tile_to_floating(state),
        Command::ToggleFloating => toggle_floating(state),

        Command::FocusNextTag => focus_tag_change(state, 1),
        Command::FocusPreviousTag => focus_tag_change(state, -1),
        Command::FocusWindow(param) => focus_window(state, param),
        Command::FocusWindowUp => move_focus_common_vars!(focus_window_change(state, -1)),
        Command::FocusWindowDown => move_focus_common_vars!(focus_window_change(state, 1)),
        Command::FocusWindowTop { swap } => focus_window_top(state, *swap),
        Command::FocusWorkspaceNext => focus_workspace_change(state, 1),
        Command::FocusWorkspacePrevious => focus_workspace_change(state, -1),

        Command::SoftReload => {
            // Make sure the currently focused window is saved for the tag.
            if let Some((handle, Some(tag))) = state
                .focus_manager
                .window(&state.windows)
                .map(|w| (w.handle, w.tag))
            {
                let old_handle = state
                    .focus_manager
                    .tags_last_window
                    .entry(tag)
                    .or_insert(handle);
                *old_handle = handle;
            }
            manager.config.save_state(&manager.state);
            manager.hard_reload();
            None
        }
        Command::HardReload => {
            manager.hard_reload();
            None
        }

        Command::RotateTag => rotate_tag(state),

        Command::IncreaseMainWidth(delta) => change_main_width(state, *delta, 1),
        Command::DecreaseMainWidth(delta) => change_main_width(state, *delta, -1),
        Command::SetMarginMultiplier(multiplier) => set_margin_multiplier(state, *multiplier),
        Command::SendWorkspaceToTag(ws_index, tag_index) => {
            Some(send_workspace_to_tag(state, *ws_index, *tag_index))
        }
        Command::CloseAllOtherWindows => close_all_other_windows(state),
        Command::Other(cmd) => Some(C::command_handler(cmd, manager)),
    }
}

fn toggle_state(state: &mut State, window_state: WindowState) -> Option<bool> {
    let window = state.focus_manager.window(&state.windows)?;
    let handle = window.handle;
    let toggle_to = !window.has_state(&window_state);
    let act = DisplayAction::SetState(handle, toggle_to, window_state);
    state.actions.push_back(act);
    state.handle_window_focus(&handle);
    match window_state {
        WindowState::Fullscreen => Some(true),
        _ => Some(false),
    }
}

fn move_to_tag<C: Config, SERVER: DisplayServer>(
    window: Option<WindowHandle>,
    tag_num: TagId,
    manager: &mut Manager<C, SERVER>,
) -> Option<bool> {
    let tag = manager.state.tags.get(tag_num)?.clone();

    // In order to apply the correct margin multiplier we want to copy this value
    // from any window already present on the target tag
    let margin_multiplier = match manager.state.windows.iter().find(|w| w.has_tag(&tag.id)) {
        Some(w) => w.margin_multiplier(),
        None => 1.0,
    };

    let handle = window.or(*manager.state.focus_manager.window_history.get(0)?)?;
    // Only handle the focus when moving the focused window.
    let handle_focus = window.is_none();
    // Focus the next or previous window on the workspace.
    let new_handle = if handle_focus {
        manager.get_next_or_previous_handle(&handle)
    } else {
        None
    };

    let window = manager
        .state
        .windows
        .iter_mut()
        .find(|w| w.handle == handle)?;

    window.untag();
    window.set_floating(false);
    window.tag(&tag.id);
    window.apply_margin_multiplier(margin_multiplier);
    let act = DisplayAction::SetWindowTag(window.handle, Some(tag.id));
    manager.state.actions.push_back(act);

    manager.state.sort_windows();
    if handle_focus {
        if let Some(new_handle) = new_handle {
            manager.state.focus_window(&new_handle);
        } else {
            let act = DisplayAction::Unfocus(Some(handle), false);
            manager.state.actions.push_back(act);
            manager.state.focus_manager.window_history.push_front(None);
        }
    }
    Some(true)
}

/// Move currently focused window to tag relative to current tag
///
/// Conditionally allow focus to follow the window to the target tag
fn move_to_tag_relative<C: Config, SERVER: DisplayServer>(
    manager: &mut Manager<C, SERVER>,
    follow: bool,
    delta: i32,
) -> Option<bool> {
    // Map indexing from 1..len to 0..(len - 1)
    let current_tag = manager.state.focus_manager.tag(0).unwrap_or_default() - 1;
    // apply euclidean division reminder to the result of offseting to wrap around tags vector
    // and add 1 to remap back to 1..len indexing
    let tags_len = manager.state.tags.normal().len() as isize;
    let desired_tag = (current_tag as isize + delta as isize).rem_euclid(tags_len) + 1;
    let desired_tag = desired_tag as usize;

    move_to_tag(None, desired_tag, manager);
    if follow {
        let moved_window = *manager.state.focus_manager.window_history.get(1)?;
        manager.state.goto_tag_handler(desired_tag);
        manager.state.handle_window_focus(&moved_window?);
    }
    Some(true)
}

fn move_window_to_workspace_change<C: Config, SERVER: DisplayServer>(
    manager: &mut Manager<C, SERVER>,
    delta: i32,
) -> Option<bool> {
    let current = manager
        .state
        .focus_manager
        .workspace(&manager.state.workspaces)?;
    let workspace =
        helpers::relative_find(&manager.state.workspaces, |w| w == current, delta, true)?.clone();

    let tag_num = workspace.tag?;
    move_to_tag(None, tag_num, manager)
}

fn goto_tag(state: &mut State, input_tag: TagId, current_tag_swap: bool) -> Option<bool> {
    let current_tag = state.focus_manager.tag(0).unwrap_or_default();
    let previous_tag = state.focus_manager.tag(1).unwrap_or_default();
    let destination_tag = if current_tag_swap && current_tag == input_tag {
        previous_tag
    } else {
        input_tag
    };
    state.goto_tag_handler(destination_tag)
}

fn return_to_last_tag(state: &mut State) -> Option<bool> {
    let previous_tag = state.focus_manager.tag(1).unwrap_or_default();
    state.goto_tag_handler(previous_tag)
}

fn focus_window(state: &mut State, param: &str) -> Option<bool> {
    match param.parse::<usize>() {
        Ok(index) if index > 0 => {
            //1-based index seems more user-friendly to me in this context
            let handle = state
                .windows
                .iter()
                .filter(|w| w.visible())
                .nth(index - 1)?
                .handle;

            state.handle_window_focus(&handle);
            None
        }
        Err(_) => focus_window_by_class(state, param),
        Ok(_) => None,
    }
}

fn focus_window_by_class(state: &mut State, window_class: &str) -> Option<bool> {
    let is_target = |w: &Window| -> bool {
        w.res_name
            .as_ref()
            .zip(w.res_class.as_ref())
            .map_or(false, |(res_name, res_class)| {
                window_class == res_name || window_class == res_class
            })
    };

    let current_window = state.focus_manager.window(&state.windows)?;
    let target_window = if is_target(current_window) {
        let previous_window_handle = state.focus_manager.window_history.get(1);
        state
            .windows
            .iter()
            .find(|w| Some(&Some(w.handle)) == previous_window_handle)
            .cloned()
    } else {
        state.windows.iter().find(|w| is_target(w)).cloned()
    }?;

    let handle = target_window.handle;

    if target_window.visible() {
        state.handle_window_focus(&handle);
        return None;
    }

    let tag_id = target_window.tag?;
    state.goto_tag_handler(tag_id)?;

    match state
        .focus_manager
        .workspace(&state.workspaces)
        .map(|ws| ws.layout)
    {
        Some(layout) if layout == Layout::Monocle || layout == Layout::MainAndDeck => {
            let mut windows = helpers::vec_extract(&mut state.windows, |w| {
                w.has_tag(&tag_id) && w.is_managed() && !w.floating()
            });

            let cycle = |wins: &mut Vec<Window>, s: &mut State| {
                let window_index = wins.iter().position(|w| w.handle == handle).unwrap_or(0);
                let _ = helpers::cycle_vec(wins, -(window_index as i32));
                s.windows.append(wins);
            };

            if layout == Layout::Monocle && windows.len() > 1 {
                cycle(&mut windows, state);
            } else if layout == Layout::MainAndDeck && windows.len() > 2 {
                let main_window = windows.remove(0);
                state.windows.push(main_window);
                cycle(&mut windows, state);
            } else {
                state.windows.append(&mut windows);
            }

            state.handle_window_focus(&handle);
            Some(true)
        }
        Some(_) => {
            state.handle_window_focus(&handle);
            Some(true)
        }
        None => None,
    }
}

/// Focus the adjacent tags, depending on the delta.
/// A delta of 1 means "next tag", a delta of -1 means "previous tag".
fn focus_tag_change(state: &mut State, delta: i8) -> Option<bool> {
    let current_tag = state.focus_manager.tag(0)?;
    let tags = state.tags.normal();
    let relative_tag_id = relative_find(tags, |tag| tag.id == current_tag, i32::from(delta), true)
        .map(|tag| tag.id)?;
    state.goto_tag_handler(relative_tag_id)
}

fn swap_tags(state: &mut State) -> Option<bool> {
    if state.workspaces.len() >= 2 && state.focus_manager.workspace_history.len() >= 2 {
        let hist_a = *state.focus_manager.workspace_history.get(0)?;
        let hist_b = *state.focus_manager.workspace_history.get(1)?;
        //Update workspace tags
        let mut temp = None;
        std::mem::swap(&mut state.workspaces.get_mut(hist_a)?.tag, &mut temp);
        std::mem::swap(&mut state.workspaces.get_mut(hist_b)?.tag, &mut temp);
        std::mem::swap(&mut state.workspaces.get_mut(hist_a)?.tag, &mut temp);
        // Update dock tags and layouts.
        state.update_static();
        state
            .layout_manager
            .update_layouts(&mut state.workspaces, state.tags.all_mut());

        return Some(true);
    }
    if state.workspaces.len() == 1 {
        let last = *state.focus_manager.tag_history.get(1).unwrap();
        return state.goto_tag_handler(last);
    }
    None
}

fn close_window(state: &mut State) -> Option<bool> {
    let window = state.focus_manager.window(&state.windows)?;
    if window.is_managed() {
        let act = DisplayAction::KillWindow(window.handle);
        state.actions.push_back(act);
    }
    None
}

fn move_to_last_workspace(state: &mut State) -> Option<bool> {
    if state.workspaces.len() >= 2 && state.focus_manager.workspace_history.len() >= 2 {
        let index = *state.focus_manager.workspace_history.get(1)?;
        let wp_tags = state.workspaces.get(index)?.tag;
        let window = state.focus_manager.window_mut(&mut state.windows)?;
        window.tag = wp_tags;
        return Some(true);
    }
    None
}

fn next_layout(state: &mut State) -> Option<bool> {
    let workspace = state.focus_manager.workspace_mut(&mut state.workspaces)?;
    let layout = state.layout_manager.next_layout(workspace);
    set_layout(layout, state)
}

fn previous_layout(state: &mut State) -> Option<bool> {
    let workspace = state.focus_manager.workspace_mut(&mut state.workspaces)?;
    let layout = state.layout_manager.previous_layout(workspace);
    set_layout(layout, state)
}

fn set_layout(layout: Layout, state: &mut State) -> Option<bool> {
    let tag_id = state.focus_manager.tag(0)?;
    // When switching to Monocle or MainAndDeck layout while in Driven
    // or ClickTo focus mode, we check if the focus is given to a visible window.
    if state.focus_manager.behaviour != FocusBehaviour::Sloppy {
        //if the currently focused window is floating, nothing will be done
        let focused_window = state.focus_manager.window_history.get(0);
        let is_focused_floating = match state
            .windows
            .iter()
            .find(|w| Some(&Some(w.handle)) == focused_window)
        {
            Some(w) => w.floating(),
            None => false,
        };
        if !is_focused_floating {
            let mut to_focus: Option<Window> = None;

            if layout == Layout::Monocle {
                to_focus = state
                    .windows
                    .iter()
                    .find(|w| w.has_tag(&tag_id) && w.is_managed() && !w.floating())
                    .cloned();
            } else if layout == Layout::MainAndDeck {
                let tags_windows = state
                    .windows
                    .iter()
                    .filter(|w| w.has_tag(&tag_id) && w.is_managed() && !w.floating())
                    .collect::<Vec<&Window>>();
                if let (Some(mw), Some(tdw)) = (tags_windows.get(0), tags_windows.get(1)) {
                    // If the focused window is the main or the top of the deck, we don't do
                    // anything.
                    if let Some(&Some(h)) = focused_window {
                        if mw.handle != h && tdw.handle != h {
                            if let Some(w) = tags_windows.get(1).copied() {
                                to_focus = Some(w.clone());
                            }
                        }
                    }
                }
            }

            if let Some(w) = to_focus {
                state.focus_window(&w.handle);
            }
        }
    }
    let workspace = state.focus_manager.workspace_mut(&mut state.workspaces)?;
    workspace.layout = layout;
    let tag = state.tags.get_mut(tag_id)?;
    match layout {
        Layout::RightWiderLeftStack | Layout::LeftWiderRightStack => {
            tag.set_layout(layout, layout.main_width());
        }
        _ => tag.set_layout(layout, workspace.main_width_percentage),
    }
    Some(true)
}

fn floating_to_tile(state: &mut State) -> Option<bool> {
    let workspace = state.focus_manager.workspace(&state.workspaces)?;
    let window = state.focus_manager.window_mut(&mut state.windows)?;
    if window.must_float() {
        return None;
    }
    //Not ideal as is_floating and must_float are connected so have to check
    //them separately
    if !window.floating() {
        return None;
    }
    let handle = window.handle;
    if window.snap_to_workspace(workspace) {
        state.sort_windows();
    }
    state.handle_window_focus(&handle);
    Some(true)
}

fn tile_to_floating(state: &mut State) -> Option<bool> {
    let width = state.default_width;
    let height = state.default_height;
    let window = state.focus_manager.window_mut(&mut state.windows)?;
    if window.must_float() {
        return None;
    }
    //Not ideal as is_floating and must_float are connected so have to check
    //them separately
    if window.floating() {
        return None;
    }

    let mut normal = window.normal;
    let offset = window.container_size.unwrap_or_default();

    normal.set_x(normal.x() + window.margin.left as i32);
    normal.set_y(normal.y() + window.margin.top as i32);
    normal.set_w(width);
    normal.set_h(height);
    let floating = normal - offset;

    window.set_floating_offsets(Some(floating));
    window.start_loc = Some(floating);
    window.set_floating(true);
    state.sort_windows();

    Some(true)
}

fn toggle_floating(state: &mut State) -> Option<bool> {
    let window = state.focus_manager.window(&state.windows)?;
    if window.floating() {
        floating_to_tile(state)
    } else {
        tile_to_floating(state)
    }
}

fn move_window_change(
    state: &mut State,
    mut handle: WindowHandle,
    layout: Option<Layout>,
    mut to_reorder: Vec<Window>,
    val: i32,
) -> Option<bool> {
    let is_handle = |x: &Window| -> bool { x.handle == handle };
    if layout == Some(Layout::Monocle) {
        handle = helpers::relative_find(&to_reorder, is_handle, -val, true)?.handle;
        let _ = helpers::cycle_vec(&mut to_reorder, val);
    } else if layout == Some(Layout::MainAndDeck) {
        if let Some(index) = to_reorder.iter().position(|x: &Window| !x.floating()) {
            let mut window_group = to_reorder.split_off(index + 1);
            if !to_reorder.iter().any(|w| w.handle == handle) {
                handle = helpers::relative_find(&window_group, is_handle, -val, true)?.handle;
            }
            let _ = helpers::cycle_vec(&mut window_group, val);
            to_reorder.append(&mut window_group);
        }
    } else {
        let _ = helpers::reorder_vec(&mut to_reorder, is_handle, val);
    }
    state.windows.append(&mut to_reorder);
    state.handle_window_focus(&handle);
    Some(true)
}

//val and layout aren't used which is a bit awkward
fn move_window_top(
    state: &mut State,
    handle: WindowHandle,
    _layout: Option<Layout>,
    mut to_reorder: Vec<Window>,
    swap: bool,
) -> Option<bool> {
    // Moves the selected window at index 0 of the window list.
    // If the selected window is already at index 0, it is sent to index 1.
    let is_handle = |x: &Window| -> bool { x.handle == handle };
    let list = &mut to_reorder;
    let len = list.len();
    let index = list.iter().position(|x| is_handle(x))?;
    let item = list.get(index)?.clone();
    list.remove(index);
    let mut new_index: usize = match index {
        0 if swap => 1,
        _ => 0,
    };
    if new_index >= len {
        new_index -= len;
    }
    list.insert(new_index, item);

    state.windows.append(&mut to_reorder);
    // focus follows the window if it was not already on top of the stack
    if index > 0 {
        state.handle_window_focus(&handle);
    }
    Some(true)
}

fn focus_window_change(
    state: &mut State,
    mut handle: WindowHandle,
    layout: Option<Layout>,
    mut to_reorder: Vec<Window>,
    val: i32,
) -> Option<bool> {
    let is_handle = |x: &Window| -> bool { x.handle == handle };
    if layout == Some(Layout::Monocle) {
        // For Monocle we want to also move windows up/down
        // Not the best solution but results
        // in desired behaviour
        handle = helpers::relative_find(&to_reorder, is_handle, -val, true)?.handle;
        let _ = helpers::cycle_vec(&mut to_reorder, val);
    } else if layout == Some(Layout::MainAndDeck) {
        let len = to_reorder.len() as i32;
        if len > 0 {
            let index = match to_reorder.iter().position(|x: &Window| !x.floating()) {
                Some(i) => {
                    if i as i32 == len - 1 {
                        i
                    } else {
                        i + 1
                    }
                }
                None => len.saturating_sub(1) as usize,
            };
            let window_group = &to_reorder[..=index];
            handle = helpers::relative_find(window_group, is_handle, -val, true)?.handle;
        }
    } else if let Some(new_focused) = helpers::relative_find(&to_reorder, is_handle, val, true) {
        handle = new_focused.handle;
    }
    state.windows.append(&mut to_reorder);
    state.handle_window_focus(&handle);
    Some(layout == Some(Layout::Monocle))
}

fn focus_window_top(state: &mut State, swap: bool) -> Option<bool> {
    let tag = state.focus_manager.tag(0)?;
    let cur = state.focus_manager.window(&state.windows).map(|w| w.handle);
    let prev = state.focus_manager.tags_last_window.get(&tag).copied();
    let next = state
        .windows
        .iter()
        .find(|x| x.tag == Some(tag) && !x.floating() && x.is_managed())
        .map(|w| w.handle);

    match (next, cur, prev) {
        (Some(next), Some(cur), Some(prev)) if next == cur && swap => {
            state.handle_window_focus(&prev);
        }
        (Some(next), Some(cur), _) if next != cur => state.handle_window_focus(&next),
        _ => {}
    }
    None
}

fn close_all_other_windows(state: &mut State) -> Option<bool> {
    let current_window: Option<WindowHandle> =
        state.focus_manager.window(&state.windows).map(|w| w.handle);
    let current_workspace = state.focus_manager.workspace(&state.workspaces);

    for window in &state.windows {
        if window.handle.ne(&current_window?)
            && current_workspace?.is_displaying(window)
            && window.r#type.ne(&WindowType::Normal)
        {
            let act = DisplayAction::KillWindow(window.handle);
            state.actions.push_back(act);
        }
    }
    Some(true)
}

fn focus_workspace_change(state: &mut State, val: i32) -> Option<bool> {
    let current = state.focus_manager.workspace(&state.workspaces)?;
    let workspace = helpers::relative_find(&state.workspaces, |w| w == current, val, true)?.clone();

    if state.focus_manager.behaviour.is_sloppy() && state.focus_manager.sloppy_mouse_follows_focus {
        let action = workspace
            .tag
            .as_ref()
            .and_then(|tag| state.focus_manager.tags_last_window.get(tag))
            .map_or_else(
                || DisplayAction::MoveMouseOverPoint(workspace.xyhw.center()),
                |h| DisplayAction::MoveMouseOver(*h, true),
            );
        state.actions.push_back(action);
    }
    state.focus_workspace(&workspace);
    None
}

fn rotate_tag(state: &mut State) -> Option<bool> {
    let tag_id = state.focus_manager.tag(0)?;
    let tag = state.tags.get_mut(tag_id)?;
    tag.rotate_layout()?;
    Some(true)
}

fn change_main_width(state: &mut State, delta: i8, factor: i8) -> Option<bool> {
    let workspace = state.focus_manager.workspace_mut(&mut state.workspaces)?;
    workspace.change_main_width(delta * factor);
    let tag_id = state.focus_manager.tag(0)?;
    let tag = state.tags.get_mut(tag_id)?;
    tag.change_main_width(delta * factor);
    Some(true)
}

fn set_margin_multiplier(state: &mut State, margin_multiplier: f32) -> Option<bool> {
    let ws = state.focus_manager.workspace_mut(&mut state.workspaces)?;
    ws.set_margin_multiplier(margin_multiplier);
    let tag = ws.tag;
    if state.windows.iter().any(|w| w.r#type == WindowType::Normal) {
        let for_active_workspace =
            |x: &Window| -> bool { tag == x.tag && x.r#type == WindowType::Normal };
        let mut to_apply_margin_multiplier =
            helpers::vec_extract(&mut state.windows, for_active_workspace);
        for w in &mut to_apply_margin_multiplier {
            if let Some(ws) = state.focus_manager.workspace(&state.workspaces) {
                w.apply_margin_multiplier(ws.margin_multiplier());
            }
        }
        state.windows.append(&mut to_apply_margin_multiplier);
    }
    Some(true)
}

fn send_workspace_to_tag(state: &mut State, ws_index: usize, tag_index: usize) -> bool {
    // todo: address inconsistency of using the index instead of the id here
    if ws_index < state.workspaces.len() && tag_index < state.tags.len_normal() {
        let workspace = &state.workspaces[ws_index].clone();
        state.focus_workspace(workspace);
        state.goto_tag_handler(tag_index + 1);
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Tags;

    #[test]
    fn return_to_last_tag_should_go_back_to_last_tag() {
        let mut manager = Manager::new_test(vec![
            "A15".to_string(),
            "B24".to_string(),
            "C".to_string(),
            "6D4".to_string(),
            "E39".to_string(),
            "F67".to_string(),
        ]);
        manager.screen_create_handler(Screen::default());
        manager.screen_create_handler(Screen::default());

        assert!(manager.command_handler(&Command::GoToTag {
            tag: 1,
            swap: false
        }));
        let current_tag = manager.state.focus_manager.tag(0).unwrap();
        assert_eq!(current_tag, 1);

        assert!(manager.command_handler(&Command::GoToTag {
            tag: 2,
            swap: false
        }));
        let current_tag = manager.state.focus_manager.tag(0).unwrap_or_default();
        assert_eq!(current_tag, 2);

        manager.command_handler(&Command::ReturnToLastTag);
        let current_tag = manager.state.focus_manager.tag(0).unwrap_or_default();
        assert_eq!(current_tag, 1);
    }

    #[test]
    fn go_to_tag_should_return_false_if_no_screen_is_created() {
        let mut manager = Manager::new_test(vec![]);
        // no screen creation here
        assert!(!manager.command_handler(&Command::GoToTag {
            tag: 6,
            swap: false
        }));
        assert!(!manager.command_handler(&Command::GoToTag {
            tag: 2,
            swap: false
        }));
        assert!(!manager.command_handler(&Command::GoToTag {
            tag: 15,
            swap: false
        }));
    }

    #[test]
    fn go_to_tag_should_create_at_least_one_tag_per_screen_no_more() {
        let mut manager = Manager::new_test(vec![]);
        manager.screen_create_handler(Screen::default());
        manager.screen_create_handler(Screen::default());
        // no tag creation here but one tag per screen is created
        assert!(manager.command_handler(&Command::GoToTag {
            tag: 2,
            swap: false
        }));
        assert!(manager.command_handler(&Command::GoToTag {
            tag: 1,
            swap: false
        }));
        // we only have one tag per screen created automatically
        assert!(!manager.command_handler(&Command::GoToTag {
            tag: 3,
            swap: false
        }));
    }

    #[test]
    fn go_to_tag_should_return_false_on_invalid_input() {
        let mut manager = Manager::new_test(vec![]);
        manager.screen_create_handler(Screen::default());
        manager.state.tags = Tags::new();
        manager.state.tags.add_new("A15", Layout::default());
        manager.state.tags.add_new("B24", Layout::default());
        manager.state.tags.add_new("C", Layout::default());
        manager.state.tags.add_new("6D4", Layout::default());
        manager.state.tags.add_new("E39", Layout::default());
        manager.state.tags.add_new("F67", Layout::default());
        assert!(!manager.command_handler(&Command::GoToTag {
            tag: 0,
            swap: false
        }));
        assert!(!manager.command_handler(&Command::GoToTag {
            tag: 999,
            swap: false
        }));
    }

    #[test]
    fn go_to_tag_should_go_to_tag_and_set_history() {
        let mut manager = Manager::new_test(vec![
            "A15".to_string(),
            "B24".to_string(),
            "C".to_string(),
            "6D4".to_string(),
            "E39".to_string(),
            "F67".to_string(),
        ]);
        manager.screen_create_handler(Screen::default());
        manager.screen_create_handler(Screen::default());

        assert!(manager.command_handler(&Command::GoToTag {
            tag: 6,
            swap: false
        }));
        let current_tag = manager.state.focus_manager.tag(0).unwrap();
        assert_eq!(current_tag, 6);

        assert!(manager.command_handler(&Command::GoToTag {
            tag: 2,
            swap: false
        }));
        let current_tag = manager.state.focus_manager.tag(0).unwrap_or_default();
        assert_eq!(current_tag, 2);

        assert!(manager.command_handler(&Command::GoToTag {
            tag: 3,
            swap: false
        }));
        let current_tag = manager.state.focus_manager.tag(0).unwrap_or_default();
        assert_eq!(current_tag, 3);

        assert!(manager.command_handler(&Command::GoToTag {
            tag: 4,
            swap: false
        }));
        let current_tag = manager.state.focus_manager.tag(0).unwrap_or_default();
        assert_eq!(current_tag, 4);

        // test tag history
        assert_eq!(manager.state.focus_manager.tag(1).unwrap_or_default(), 3);
        assert_eq!(manager.state.focus_manager.tag(2).unwrap_or_default(), 2);
        assert_eq!(manager.state.focus_manager.tag(3).unwrap_or_default(), 6);
    }

    #[test]
    fn focus_tag_change_should_go_to_previous_and_next_tag() {
        let mut manager = Manager::new_test(vec![
            "A15".to_string(),
            "B24".to_string(),
            "C".to_string(),
            "6D4".to_string(),
            "E39".to_string(),
            "F67".to_string(),
        ]);
        manager.screen_create_handler(Screen::default());
        let state = &mut manager.state;

        state.focus_tag(&2);
        assert_eq!(state.focus_manager.tag(0).unwrap(), 2);

        focus_tag_change(state, 1);
        assert_eq!(state.focus_manager.tag(0).unwrap(), 3);

        focus_tag_change(state, -1);
        assert_eq!(state.focus_manager.tag(0).unwrap(), 2);

        focus_tag_change(state, 2);
        assert_eq!(state.focus_manager.tag(0).unwrap(), 4);

        focus_tag_change(state, -5);
        assert_eq!(state.focus_manager.tag(0).unwrap(), 5);

        focus_tag_change(state, 3);
        assert_eq!(state.focus_manager.tag(0).unwrap(), 2);

        focus_tag_change(state, 13);
        assert_eq!(state.focus_manager.tag(0).unwrap(), 3);
    }

    #[test]
    fn focus_window_top() {
        let mut manager = Manager::new_test(vec![]);
        manager.screen_create_handler(Screen::default());

        manager.window_created_handler(
            Window::new(WindowHandle::MockHandle(1), None, None),
            -1,
            -1,
        );
        manager.window_created_handler(
            Window::new(WindowHandle::MockHandle(2), None, None),
            -1,
            -1,
        );
        manager.window_created_handler(
            Window::new(WindowHandle::MockHandle(3), None, None),
            -1,
            -1,
        );

        let expected = manager.state.windows[0].clone();
        let initial = manager.state.windows[1].clone();

        manager.state.focus_window(&initial.handle);

        manager.command_handler(&Command::FocusWindowTop { swap: false });
        let actual = manager
            .state
            .focus_manager
            .window(&manager.state.windows)
            .unwrap()
            .handle;
        assert_eq!(expected.handle, actual);

        manager.command_handler(&Command::FocusWindowTop { swap: false });
        let actual = manager
            .state
            .focus_manager
            .window(&manager.state.windows)
            .unwrap()
            .handle;
        assert_eq!(expected.handle, actual);

        manager.command_handler(&Command::FocusWindowTop { swap: true });
        let actual = manager
            .state
            .focus_manager
            .window(&manager.state.windows)
            .unwrap()
            .handle;
        assert_eq!(initial.handle, actual);
    }

    #[test]
    fn move_window_top() {
        let mut manager = Manager::new_test(vec![]);
        manager.screen_create_handler(Screen::default());

        manager.window_created_handler(
            Window::new(WindowHandle::MockHandle(1), None, None),
            -1,
            -1,
        );
        manager.window_created_handler(
            Window::new(WindowHandle::MockHandle(2), None, None),
            -1,
            -1,
        );
        manager.window_created_handler(
            Window::new(WindowHandle::MockHandle(3), None, None),
            -1,
            -1,
        );

        let expected = manager.state.windows[0].clone();
        let initial = manager.state.windows[1].clone();

        manager.state.focus_window(&initial.handle);

        manager.command_handler(&Command::MoveWindowTop { swap: false });
        assert_eq!(manager.state.windows[0].handle, initial.handle);

        manager.command_handler(&Command::MoveWindowTop { swap: false });
        assert_eq!(manager.state.windows[0].handle, initial.handle);

        manager.command_handler(&Command::MoveWindowTop { swap: true });
        assert_eq!(manager.state.windows[0].handle, expected.handle);
    }

    #[test]
    fn move_window_to_next_or_prev_tag_should_be_able_to_cycle() {
        let mut manager = Manager::new_test(vec![
            "AO".to_string(),
            "EU".to_string(),
            "ID".to_string(),
            "HT".to_string(),
            "NS".to_string(),
        ]);
        manager.screen_create_handler(Screen::default());
        manager.window_created_handler(
            Window::new(WindowHandle::MockHandle(1), None, None),
            -1,
            -1,
        );

        let first_tag = manager.state.tags.get(1).unwrap().id;
        let third_tag = manager.state.tags.get(3).unwrap().id;
        let last_tag = manager.state.tags.get(5).unwrap().id;

        assert!(manager.state.windows[0].has_tag(&first_tag));

        manager.command_handler(&Command::MoveWindowToPreviousTag { follow: true });
        assert!(manager.state.windows[0].has_tag(&last_tag));

        (0..3).for_each(|_| {
            manager.command_handler(&Command::MoveWindowToNextTag { follow: false });
            manager.command_handler(&Command::FocusNextTag);
        });
        assert!(manager.state.windows[0].has_tag(&third_tag));
    }

    #[test]
    fn move_window_to_next_or_prev_tag_should_be_able_to_keep_window_focused() {
        let mut manager = Manager::new_test(vec!["AO".to_string(), "EU".to_string()]);
        manager.screen_create_handler(Screen::default());
        manager.window_created_handler(
            Window::new(WindowHandle::MockHandle(1), None, None),
            -1,
            -1,
        );
        manager.window_created_handler(
            Window::new(WindowHandle::MockHandle(2), None, None),
            -1,
            -1,
        );
        let expected_tag = manager.state.tags.get(2).unwrap().id;
        manager.command_handler(&Command::SendWindowToTag {
            window: None,
            tag: expected_tag,
        });
        let initial = manager.state.windows[0].clone();

        manager.command_handler(&Command::MoveWindowToNextTag { follow: true });

        assert_eq!(
            *manager.state.focus_manager.tag_history.get(0).unwrap(),
            expected_tag
        );
        assert_eq!(manager.state.windows[0].handle, initial.handle);
    }
}
