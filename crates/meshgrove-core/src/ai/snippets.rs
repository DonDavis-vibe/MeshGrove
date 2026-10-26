//! What users paste into their AI client so it starts `meshgrove-mcp`, for setting one up by
//! hand. Meshgrove connects the assistants it finds by itself ([`super::agent_setup`]).

use std::path::{Path, PathBuf};

use serde_json::json;

use crate::config::AccessClass;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Client {
    ClaudeDesktop,
    ClaudeCode,
    LmStudio,
    Cursor,
    VsCode,
}

impl Client {
    pub const ALL: [Client; 5] =
        [Client::ClaudeDesktop, Client::ClaudeCode, Client::LmStudio, Client::Cursor, Client::VsCode];

    pub fn name(self) -> &'static str {
        match self {
            Client::ClaudeDesktop => "Claude Desktop",
            Client::ClaudeCode => "Claude Code",
            Client::LmStudio => "LM Studio",
            Client::Cursor => "Cursor",
            Client::VsCode => "VS Code",
        }
    }

    /// The name `meshgrove-mcp --client` gets, for the access log.
    pub fn id(self) -> &'static str {
        match self {
            Client::ClaudeDesktop => "claude-desktop",
            Client::ClaudeCode => "claude-code",
            Client::LmStudio => "lm-studio",
            Client::Cursor => "cursor",
            Client::VsCode => "vscode",
        }
    }

    /// Where the client keeps its MCP servers (the snippet goes there); empty for a command.
    pub fn config_files(self) -> Vec<PathBuf> {
        let config = dirs::config_dir();
        let home = dirs::home_dir();
        match self {
            Client::ClaudeDesktop => {
                config.map(|c| c.join("Claude").join("claude_desktop_config.json")).into_iter().collect()
            }
            Client::LmStudio => home.map(|h| h.join(".lmstudio").join("mcp.json")).into_iter().collect(),
            Client::Cursor => home.map(|h| h.join(".cursor").join("mcp.json")).into_iter().collect(),
            Client::VsCode => config.map(|c| c.join("Code").join("User").join("mcp.json")).into_iter().collect(),
            Client::ClaudeCode => Vec::new(),
        }
    }

    /// A shell command to run, not a file to edit.
    pub fn is_command(self) -> bool {
        self == Client::ClaudeCode
    }
}

/// The program and arguments that start the server. An AppImage (`appimage`) starts it as
/// `<AppImage> mcp …`, since the binary inside has another path on every run.
pub fn command(exe: &Path, appimage: Option<&Path>, class: AccessClass, client: Client) -> (String, Vec<String>) {
    let server = super::agent_setup::Server::new(exe, appimage);
    let args = server.args(client.id(), class == AccessClass::Local);
    (server.program, args)
}

/// What to paste: JSON for the clients' config files, a command for Claude Code.
pub fn snippet(client: Client, exe: &Path, appimage: Option<&Path>, class: AccessClass) -> String {
    let (program, args) = command(exe, appimage, class, client);
    let value = match client {
        Client::ClaudeCode => {
            let mut words = vec!["claude".to_owned(), "mcp".into(), "add".into(), "--scope".into(), "user".into()];
            words.extend(["meshgrove".to_owned(), "--".into(), program]);
            words.extend(args);
            return words.iter().map(|w| shell_quote(w, cfg!(windows))).collect::<Vec<_>>().join(" ");
        }
        Client::VsCode => json!({ "servers": { "meshgrove": { "type": "stdio", "command": program, "args": args } } }),
        _ => json!({ "mcpServers": { "meshgrove": { "command": program, "args": args } } }),
    };
    serde_json::to_string_pretty(&value).unwrap_or_default()
}

/// A word for the shell the command is pasted into: POSIX shells take single quotes; on Windows
/// cmd does not, so double quotes, which PowerShell takes too (Windows paths hold no `"`).
fn shell_quote(word: &str, windows: bool) -> String {
    if !word.is_empty() && word.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:=@".contains(c)) {
        word.to_owned()
    } else if windows {
        format!("\"{word}\"")
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippets_are_valid_json_with_the_right_arguments() {
        let exe = Path::new(r"C:\Program Files\Meshgrove\meshgrove-mcp.exe");
        let text = snippet(Client::ClaudeDesktop, exe, None, AccessClass::Local);
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        let server = &v["mcpServers"]["meshgrove"];
        assert_eq!(server["command"], r"C:\Program Files\Meshgrove\meshgrove-mcp.exe", "backslashes survive");
        assert_eq!(server["args"], json!(["--local", "--client", "claude-desktop"]));
        let vscode: serde_json::Value = serde_json::from_str(&snippet(
            Client::VsCode,
            Path::new("/usr/bin/meshgrove-mcp"),
            None,
            AccessClass::Cloud,
        ))
        .unwrap();
        assert_eq!(vscode["servers"]["meshgrove"]["type"], "stdio");
        assert_eq!(vscode["servers"]["meshgrove"]["args"], json!(["--client", "vscode"]), "cloud is the default");
    }

    #[test]
    fn claude_code_gets_a_command_and_appimages_a_subcommand() {
        let cmd = snippet(
            Client::ClaudeCode,
            Path::new("/tmp/.mount_x/usr/bin/meshgrove-mcp"),
            Some(Path::new("/home/davis/Apps/Meshgrove 0.5.AppImage")),
            AccessClass::Local,
        );
        let image = if cfg!(windows) {
            "\"/home/davis/Apps/Meshgrove 0.5.AppImage\""
        } else {
            "'/home/davis/Apps/Meshgrove 0.5.AppImage'"
        };
        assert_eq!(cmd, format!("claude mcp add --scope user meshgrove -- {image} mcp --local --client claude-code"));
        assert!(Client::ClaudeDesktop.config_files()[0].ends_with("Claude/claude_desktop_config.json"));
    }

    #[test]
    fn windows_commands_quote_for_cmd_and_powershell() {
        let exe = r"C:\Program Files\Meshgrove\meshgrove-mcp.exe";
        assert_eq!(shell_quote(exe, true), format!("\"{exe}\""));
        assert_eq!(shell_quote("--local", true), "--local");
        assert_eq!(shell_quote("it's", false), r"'it'\''s'");
    }
}
