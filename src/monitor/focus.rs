// Focus feature - uses Accessibility API for Ghostty tab focus on macOS

use std::process::Command;

#[cfg(target_os = "macos")]
pub mod ax {
    use accessibility_sys::{
        AXUIElementCopyAttributeValue, AXUIElementCreateApplication, AXUIElementPerformAction,
        AXUIElementRef, kAXChildrenAttribute, kAXErrorSuccess, kAXRoleAttribute, kAXTitleAttribute,
    };
    use core_foundation::{
        array::CFArray,
        base::{CFType, TCFType},
        string::CFString,
    };
    use std::ptr;

    pub fn get_ghostty_pid() -> Option<i32> {
        let output = std::process::Command::new("pgrep")
            .arg("-x")
            .arg("ghostty")
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        String::from_utf8_lossy(&output.stdout)
            .trim()
            .lines()
            .next()?
            .parse()
            .ok()
    }

    pub fn focus_ghostty_tab_by_title(target_title: &str) -> Result<bool, String> {
        let pid = match get_ghostty_pid() {
            Some(pid) => pid,
            None => return Ok(false), // Ghostty not running
        };

        unsafe {
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() {
                return Err("Failed to create AXUIElement for Ghostty".into());
            }

            // Search for tab with matching title
            let result = find_and_focus_tab(app, target_title);

            // Release app element
            core_foundation::base::CFRelease(app as *const _);

            result
        }
    }

    unsafe fn find_and_focus_tab(app: AXUIElementRef, target_title: &str) -> Result<bool, String> {
        // Get windows
        let windows = unsafe { get_children(app)? };

        for i in 0..windows.len() {
            let window: AXUIElementRef =
                unsafe { std::mem::transmute(windows.get(i).unwrap().as_CFTypeRef()) };

            // Recursively search for tabs in this window
            if unsafe { search_and_focus_tab(window, target_title)? } {
                return Ok(true);
            }
        }

        Ok(false)
    }

    unsafe fn search_and_focus_tab(
        element: AXUIElementRef,
        target_title: &str,
    ) -> Result<bool, String> {
        // Check if this element is a tab (AXRadioButton in tab group)
        if let Some(role) = unsafe { get_role(element) } {
            if role == "AXRadioButton" || role == "AXButton" {
                if let Some(title) = unsafe { get_title(element) } {
                    // Match if either contains the other (bidirectional matching)
                    // e.g., tab "tank" matches search "tank-workspace" and vice versa
                    if title.contains(target_title) || target_title.contains(&title) {
                        // Found matching tab, press it
                        let action = CFString::new("AXPress");
                        let err = unsafe {
                            AXUIElementPerformAction(element, action.as_concrete_TypeRef())
                        };
                        if err == kAXErrorSuccess {
                            return Ok(true);
                        }
                    }
                }
            }
        }

        // Recursively search children
        if let Ok(children) = unsafe { get_children(element) } {
            for i in 0..children.len() {
                let child: AXUIElementRef =
                    unsafe { std::mem::transmute(children.get(i).unwrap().as_CFTypeRef()) };
                if unsafe { search_and_focus_tab(child, target_title)? } {
                    return Ok(true);
                }
            }
        }

        Ok(false)
    }

    unsafe fn get_children(element: AXUIElementRef) -> Result<CFArray<CFType>, String> {
        let attr = CFString::from_static_string(kAXChildrenAttribute);
        let mut value: core_foundation::base::CFTypeRef = ptr::null();

        let err = unsafe {
            AXUIElementCopyAttributeValue(element, attr.as_concrete_TypeRef(), &mut value)
        };
        if err != kAXErrorSuccess || value.is_null() {
            return Err("Failed to get children".into());
        }

        Ok(unsafe { CFArray::wrap_under_create_rule(value as *const _) })
    }

    unsafe fn get_role(element: AXUIElementRef) -> Option<String> {
        let attr = CFString::from_static_string(kAXRoleAttribute);
        let mut value: core_foundation::base::CFTypeRef = ptr::null();

        let err = unsafe {
            AXUIElementCopyAttributeValue(element, attr.as_concrete_TypeRef(), &mut value)
        };
        if err != kAXErrorSuccess || value.is_null() {
            return None;
        }

        let cf_str = unsafe { CFString::wrap_under_create_rule(value as *const _) };
        Some(cf_str.to_string())
    }

    unsafe fn get_title(element: AXUIElementRef) -> Option<String> {
        let attr = CFString::from_static_string(kAXTitleAttribute);
        let mut value: core_foundation::base::CFTypeRef = ptr::null();

        let err = unsafe {
            AXUIElementCopyAttributeValue(element, attr.as_concrete_TypeRef(), &mut value)
        };
        if err != kAXErrorSuccess || value.is_null() {
            return None;
        }

        let cf_str = unsafe { CFString::wrap_under_create_rule(value as *const _) };
        Some(cf_str.to_string())
    }

    /// Dump the UI tree for debugging
    pub fn dump_ghostty_ui_tree() -> Result<String, String> {
        let pid = match get_ghostty_pid() {
            Some(pid) => pid,
            None => return Err("Ghostty not running".into()),
        };

        unsafe {
            let app = AXUIElementCreateApplication(pid);
            if app.is_null() {
                return Err("Failed to create AXUIElement for Ghostty".into());
            }

            let mut output = String::new();
            dump_element(app, 0, &mut output);

            core_foundation::base::CFRelease(app as *const _);
            Ok(output)
        }
    }

    unsafe fn dump_element(element: AXUIElementRef, depth: usize, output: &mut String) {
        let indent = "  ".repeat(depth);
        let role = unsafe { get_role(element) }.unwrap_or_else(|| "?".into());
        let title = unsafe { get_title(element) }.unwrap_or_default();

        output.push_str(&format!("{}{}", indent, role));
        if !title.is_empty() {
            output.push_str(&format!(" [{}]", title));
        }
        output.push('\n');

        // Limit depth to avoid infinite recursion
        if depth > 10 {
            return;
        }

        if let Ok(children) = unsafe { get_children(element) } {
            for i in 0..children.len() {
                let child: AXUIElementRef =
                    unsafe { std::mem::transmute(children.get(i).unwrap().as_CFTypeRef()) };
                unsafe { dump_element(child, depth + 1, output) };
            }
        }
    }
}

/// Focus a Ghostty tab by matching the project name in the tab title.
/// Uses macOS Accessibility API to find and click the matching tab.
#[cfg(target_os = "macos")]
pub fn focus_ghostty_tab(project_name: &str) -> Result<bool, Box<dyn std::error::Error>> {
    match ax::focus_ghostty_tab_by_title(project_name) {
        Ok(found) => {
            if found {
                // Bring Ghostty to front
                let _ = Command::new("open").arg("-a").arg("Ghostty").status();
            }
            Ok(found)
        }
        Err(e) => Err(e.into()),
    }
}

/// Focus a Ghostty tab by TTY, using tmux session name as tab title.
#[cfg(target_os = "macos")]
pub fn focus_ghostty_tab_by_tty(tty: &str) -> Result<bool, Box<dyn std::error::Error>> {
    // Try to get tmux session name from TTY
    if let Some(session_name) = get_tmux_session_for_tty(tty) {
        // First select the tmux pane
        let _ = select_tmux_pane(tty);
        // Then focus the Ghostty tab by session name
        return focus_ghostty_tab(&session_name);
    }

    // Fallback: try to use TTY directly (non-tmux case)
    Ok(false)
}

/// Get tmux session name for a given TTY
fn get_tmux_session_for_tty(tty: &str) -> Option<String> {
    // tmux list-panes -a -F "#{pane_tty}|#{session_name}|#{window_index}|#{pane_index}"
    let output = Command::new("tmux")
        .args([
            "list-panes",
            "-a",
            "-F",
            "#{pane_tty}|#{session_name}|#{window_index}|#{pane_index}",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() >= 2 && parts[0] == tty {
            let session_name = parts[1];
            // Strip numeric suffix (e.g., "private-8" -> "private")
            return Some(strip_numeric_suffix(session_name));
        }
    }

    None
}

/// Strip numeric suffix from tmux session name (e.g., "private-8" -> "private")
fn strip_numeric_suffix(name: &str) -> String {
    if let Some(idx) = name.rfind('-') {
        let suffix = &name[idx + 1..];
        if suffix.chars().all(|c| c.is_ascii_digit()) {
            return name[..idx].to_string();
        }
    }
    name.to_string()
}

/// Select tmux pane by TTY
fn select_tmux_pane(tty: &str) -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new("tmux")
        .args([
            "list-panes",
            "-a",
            "-F",
            "#{pane_tty}|#{session_name}|#{window_index}|#{pane_index}",
        ])
        .output()?;

    if !output.status.success() {
        return Err("tmux not running".into());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() >= 4 && parts[0] == tty {
            let session = parts[1];
            let window = parts[2];
            let pane = parts[3];

            // Select the window and pane
            let target = format!("{}:{}.{}", session, window, pane);
            Command::new("tmux")
                .args(["select-window", "-t", &format!("{}:{}", session, window)])
                .status()?;
            Command::new("tmux")
                .args(["select-pane", "-t", &target])
                .status()?;

            return Ok(());
        }
    }

    Err("TTY not found in tmux".into())
}

#[cfg(not(target_os = "macos"))]
pub fn focus_ghostty_tab(_project_name: &str) -> Result<bool, Box<dyn std::error::Error>> {
    Err("Ghostty tab focus is only supported on macOS".into())
}
