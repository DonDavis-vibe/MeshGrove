#!/usr/bin/env python3
"""A stand-in for an AI agent, to see the chat's cards without a real one.

It speaks the Agent Client Protocol on its standard input and output, as Claude Code, Codex and
the others do when Meshgrove starts them, and answers by a few words in the message:

    permission   asks whether it may run a command (the approval card)
    sign in      says it needs a sign-in (the sign-in card); "Sign in with the browser" then works
    warn         stops its answer early (a warning under the answer)
    fail         fails (the answer did not come through)
    anything else: a short answer with two tools in its list

German words work too: erlaubnis, anmelden, warnung, fehler.

It runs nothing, reads nothing and talks to no one: every answer is made up here.
`scripts/chat-demo.py` starts a Meshgrove of its own with this agent set up.
"""

import json
import sys
import time

SESSION = "demo-session"

AUTH_METHODS = [
    {"id": "browser", "name": "Sign in with the browser", "description": "The demo signs in at once."},
    {"id": "terminal", "name": "Sign in in a terminal", "type": "terminal", "args": ["login"]},
]

HELP = (
    "I am a stand-in agent. Write one of these words to see a card:\n\n"
    "- **permission**: I ask whether I may run a command\n"
    "- **sign in**: I say that I need a sign-in\n"
    "- **warn**: I stop my answer early\n"
    "- **fail**: my answer does not come through\n"
)


def send(message: dict) -> None:
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


def reply(rpc, result: dict) -> None:
    send({"jsonrpc": "2.0", "id": rpc, "result": result})


def fail(rpc, code: int, message: str) -> None:
    send({"jsonrpc": "2.0", "id": rpc, "error": {"code": code, "message": message}})


def update(body: dict) -> None:
    send({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": SESSION, "update": body}})


def say(text: str) -> None:
    """The answer, a few words at a time, as an agent streams it."""
    words = text.split(" ")
    for start in range(0, len(words), 4):
        chunk = " ".join(words[start:start + 4])
        if start + 4 < len(words):
            chunk += " "
        update({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": chunk}})
        time.sleep(0.05)


def tool(call: str, title: str, kind: str, output: str) -> None:
    update({"sessionUpdate": "tool_call", "toolCallId": call, "title": title, "kind": kind, "status": "pending"})
    time.sleep(0.2)
    update({
        "sessionUpdate": "tool_call_update",
        "toolCallId": call,
        "status": "completed",
        "content": [{"type": "content", "content": {"type": "text", "text": output}}],
    })


def has(text: str, *words: str) -> bool:
    return any(word in text for word in words)


def main() -> int:
    signed_in = True
    next_request = 1000
    # The message being answered while a permission question is open: (prompt id, question id).
    waiting = None

    for line in sys.stdin:
        try:
            message = json.loads(line)
        except ValueError:
            continue
        method, rpc, params = message.get("method"), message.get("id"), message.get("params") or {}

        # The answer to our permission question.
        if method is None and waiting and rpc == waiting[1]:
            outcome = (message.get("result") or {}).get("outcome") or {}
            picked = outcome.get("optionId") if outcome.get("outcome") == "selected" else None
            update({"sessionUpdate": "tool_call_update", "toolCallId": "run-1",
                    "status": "completed" if picked in ("allow", "always") else "failed"})
            say(f"You answered: **{picked or 'cancelled'}**. Nothing was run, this is a demo.")
            reply(waiting[0], {"stopReason": "end_turn"})
            waiting = None
        elif method == "initialize":
            reply(rpc, {
                "protocolVersion": params.get("protocolVersion", 1),
                "agentCapabilities": {"promptCapabilities": {"image": False}},
                "agentInfo": {"name": "demo-agent", "title": "Demo agent", "version": "1"},
                "authMethods": AUTH_METHODS,
            })
        elif method == "authenticate":
            signed_in = True
            reply(rpc, {})
        elif method == "session/new":
            if signed_in:
                reply(rpc, {"sessionId": SESSION})
            else:
                fail(rpc, -32000, "Authentication required")
        elif method == "session/prompt":
            text = " ".join(part.get("text", "") for part in params.get("prompt", []) if part.get("type") == "text")
            # Only what the user typed now counts: in a conversation taken up again, Meshgrove
            # puts what was said earlier in front of it (and that names every word above).
            text = text.rsplit("Now the user says:", 1)[-1].lower()
            if has(text, "permission", "erlaubnis"):
                update({"sessionUpdate": "tool_call", "toolCallId": "run-1", "title": "rm -rf build/",
                        "kind": "execute", "status": "pending"})
                next_request += 1
                waiting = (rpc, next_request)
                send({"jsonrpc": "2.0", "id": next_request, "method": "session/request_permission", "params": {
                    "sessionId": SESSION,
                    "toolCall": {"toolCallId": "run-1", "title": "rm -rf build/", "kind": "execute"},
                    "options": [
                        {"optionId": "allow", "name": "Allow once", "kind": "allow_once"},
                        {"optionId": "always", "name": "Always allow", "kind": "allow_always"},
                        {"optionId": "reject", "name": "Reject", "kind": "reject_once"},
                    ],
                }})
            elif has(text, "sign in", "sign-in", "signin", "anmeld"):
                signed_in = False
                fail(rpc, -32000, "Authentication required")
            elif has(text, "warn"):
                say("This answer stops in the middle of its")
                reply(rpc, {"stopReason": "max_tokens"})
            elif has(text, "fail", "fehl"):
                say("I start an answer, and then")
                fail(rpc, -32603, "The demo agent failed on purpose.")
            else:
                tool("look-1", "search_models", "search", "12 files with the tag garden")
                tool("look-2", "check_printability", "other", "3 of 12 fit the bed")
                say(HELP)
                reply(rpc, {"stopReason": "end_turn"})
        elif method == "session/cancel":
            if waiting:
                reply(waiting[0], {"stopReason": "cancelled"})
                waiting = None
        elif rpc is not None and method is not None:
            fail(rpc, -32601, f"The demo agent does not serve `{method}`")
    return 0


if __name__ == "__main__":
    sys.exit(main())
