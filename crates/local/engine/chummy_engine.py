"""Chummy's built-in local engine.

A deliberately small OpenAI-compatible server around Hugging Face
`transformers`. It loads one model with `from_pretrained`, generates one answer
at a time, and turns the model's tool-call text into OpenAI `tool_calls`.

It is much simpler (and slower) than vLLM, llama.cpp or Ollama: no batching,
no paged attention, no speculative decoding. It exists so a model can be run
straight from a Hugging Face repo with no other software installed.

Security:
  * listens on 127.0.0.1 only, and every request must carry the bearer token
    Chummy generated for this run;
  * runs fully offline (HF_HUB_OFFLINE) from a folder Chummy downloaded;
  * never sets trust_remote_code, and only loads safetensors weights.

Only the Python standard library is needed at import time, so the parsers and
the `--fake` backend can be tested without torch.

This file is part of Chummy (MIT license).
"""

from __future__ import annotations

import argparse
import hmac
import json
import os
import re
import sys
import threading
import time
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Iterable, Iterator

# --------------------------------------------------------------------------
# Tool-call parsers
#
# Each parser knows the text that starts a tool-call section ("markers") and
# how to turn the model's full output into (visible_text, calls). A call's
# "arguments" is a JSON string; if the model wrote invalid JSON the raw text is
# passed through and Chummy reports it as a malformed call.
# --------------------------------------------------------------------------


def _call(name: str, args) -> dict:
    if isinstance(args, str):
        raw = args
    else:
        raw = json.dumps(args if args is not None else {}, ensure_ascii=False)
    return {"name": str(name), "arguments": raw}


def _json_or_raw(text: str):
    try:
        return json.loads(text)
    except (ValueError, TypeError):
        return None


def _calls_from_obj(obj) -> list[dict]:
    """Accepts {"name", "arguments"|"parameters"} or a list of them."""
    items = obj if isinstance(obj, list) else [obj]
    calls = []
    for it in items:
        if isinstance(it, dict) and "name" in it:
            args = it.get("arguments", it.get("parameters", {}))
            calls.append(_call(it["name"], args))
    return calls


class Parser:
    name = "none"
    markers: tuple[str, ...] = ()

    def parse(self, text: str) -> tuple[str, list[dict]]:
        return text, []


class HermesParser(Parser):
    """<tool_call>{"name": ..., "arguments": {...}}</tool_call> (Qwen, Hermes, ...)."""

    name = "hermes"
    markers = ("<tool_call>",)
    _re = re.compile(r"<tool_call>\s*(.*?)\s*(?:</tool_call>|$)", re.S)

    def parse(self, text):
        start = text.find("<tool_call>")
        if start < 0:
            return text, []
        calls = []
        for body in self._re.findall(text[start:]):
            obj = _json_or_raw(body)
            if obj is None:
                m = re.search(r'"name"\s*:\s*"([^"]+)"', body)
                calls.append(_call(m.group(1) if m else "", body))
            else:
                calls.extend(_calls_from_obj(obj))
        return text[:start].rstrip(), calls


class MistralParser(Parser):
    """[TOOL_CALLS][{...}] (older) or [TOOL_CALLS]name[ARGS]{...} (newer)."""

    name = "mistral"
    markers = ("[TOOL_CALLS]",)

    def parse(self, text):
        start = text.find("[TOOL_CALLS]")
        if start < 0:
            return text, []
        calls = []
        for part in text[start:].split("[TOOL_CALLS]"):
            part = part.strip()
            if not part:
                continue
            if "[ARGS]" in part:
                name, _, args = part.partition("[ARGS]")
                obj = _json_or_raw(args.strip())
                calls.append(_call(name.strip(), obj if obj is not None else args.strip()))
            else:
                obj = _json_or_raw(part)
                calls.extend(_calls_from_obj(obj) if obj is not None else [_call("", part)])
        return text[:start].rstrip(), calls


class Llama3JsonParser(Parser):
    """Llama 3.x: optional <|python_tag|>, then {"name", "parameters"} objects
    separated by ';'."""

    name = "llama3_json"
    markers = ("<|python_tag|>",)

    def parse(self, text):
        stripped = text.strip()
        if "<|python_tag|>" in stripped:
            before, _, body = stripped.partition("<|python_tag|>")
        elif stripped.startswith("{") and '"name"' in stripped:
            before, body = "", stripped
        else:
            return text, []
        calls = []
        for chunk in _split_top_level(body, ";"):
            obj = _json_or_raw(chunk)
            if obj is None:
                calls.append(_call("", chunk))
            else:
                calls.extend(_calls_from_obj(obj))
        return before.rstrip(), calls


class GraniteParser(Parser):
    """<|tool_call|>[{...}] (Granite, Phi-4-mini)."""

    name = "granite"
    markers = ("<|tool_call|>",)

    def parse(self, text):
        start = text.find("<|tool_call|>")
        if start < 0:
            return text, []
        body = text[start + len("<|tool_call|>"):].replace("<|/tool_call|>", "").strip()
        obj = _json_or_raw(body)
        calls = _calls_from_obj(obj) if obj is not None else [_call("", body)]
        return text[:start].rstrip(), calls


def _split_top_level(s: str, sep: str) -> list[str]:
    """Splits on `sep` outside of JSON strings/brackets."""
    out, depth, cur, in_str, esc = [], 0, [], False, False
    for ch in s:
        if in_str:
            cur.append(ch)
            if esc:
                esc = False
            elif ch == "\\":
                esc = True
            elif ch == '"':
                in_str = False
            continue
        if ch == '"':
            in_str = True
        elif ch in "{[":
            depth += 1
        elif ch in "}]":
            depth -= 1
        if ch == sep and depth == 0:
            out.append("".join(cur).strip())
            cur = []
        else:
            cur.append(ch)
    if "".join(cur).strip():
        out.append("".join(cur).strip())
    return [c for c in out if c]


PARSERS = {p.name: p for p in (HermesParser(), MistralParser(), Llama3JsonParser(), GraniteParser())}


# --------------------------------------------------------------------------
# Streaming splitter: routes generated text into reasoning / visible text /
# a held-back tool-call section, without ever showing a marker to the user.
# --------------------------------------------------------------------------


class StreamSplitter:
    THINK_OPEN, THINK_CLOSE = "<think>", "</think>"

    def __init__(self, parser: Parser):
        self.parser = parser
        self.full = ""  # everything after reasoning, for final parsing
        self.pending = ""  # text not yet emitted (could start a marker)
        self.in_think = False
        self.in_tools = False
        self.seen_visible = False
        self.held = ""  # the tool-call section, shown as text if it holds no call

    def _markers(self):
        ms = list(self.parser.markers)
        if self.parser.name == "llama3_json" and not self.seen_visible:
            ms.append("{")  # Llama may answer with a bare JSON call
        return ms

    def feed(self, piece: str) -> list[tuple[str, str]]:
        """Returns a list of ("reasoning"|"text", chunk)."""
        out = []
        self.pending += piece
        while self.pending:
            if self.in_think:
                i = self.pending.find(self.THINK_CLOSE)
                if i < 0:
                    keep = _partial_suffix(self.pending, [self.THINK_CLOSE])
                    emit = self.pending[: len(self.pending) - keep]
                    if emit:
                        out.append(("reasoning", emit))
                    self.pending = self.pending[len(emit):]
                    break
                if i:
                    out.append(("reasoning", self.pending[:i]))
                self.pending = self.pending[i + len(self.THINK_CLOSE):].lstrip("\n")
                self.in_think = False
                continue
            if self.in_tools:
                self.full += self.pending
                self.held += self.pending
                self.pending = ""
                break
            if not self.seen_visible and self.pending.lstrip().startswith(self.THINK_OPEN):
                self.pending = self.pending.lstrip()[len(self.THINK_OPEN):]
                self.in_think = True
                continue
            markers = self._markers()
            hits = [(self.pending.find(m), m) for m in markers if m in self.pending]
            if hits:
                i, _ = min(hits)
                before = self.pending[:i]
                if before.strip() or (before and self.seen_visible):
                    out.append(("text", before))
                self.full += self.pending
                self.held = self.pending[i:]
                self.pending = ""
                self.in_tools = True
                break
            keep = _partial_suffix(self.pending, markers + ([self.THINK_OPEN] if not self.seen_visible else []))
            emit = self.pending[: len(self.pending) - keep]
            if emit:
                if emit.strip():
                    self.seen_visible = True
                if self.seen_visible:
                    out.append(("text", emit))
                self.full += emit
            self.pending = self.pending[len(emit):]
            break
        return out

    def finish(self) -> tuple[list[tuple[str, str]], list[dict]]:
        out = []
        if self.pending and not self.in_tools and not self.in_think:
            out.append(("text", self.pending))
        self.full += self.pending if not self.in_think else ""
        self.pending = ""
        if not self.in_tools:
            return out, []
        _, calls = self.parser.parse(self.full)
        if not calls and self.held.strip():
            # It looked like a tool call (a Llama answer that starts with "{"),
            # but wasn't one: it's the answer.
            out.append(("text", self.held))
        return out, calls


def _partial_suffix(s: str, markers: Iterable[str]) -> int:
    """Length of the longest suffix of s that is a proper prefix of a marker."""
    best = 0
    for m in markers:
        for n in range(min(len(m) - 1, len(s)), 0, -1):
            if s.endswith(m[:n]):
                best = max(best, n)
                break
    return best


# --------------------------------------------------------------------------
# Backends
# --------------------------------------------------------------------------


def read_context_length(model_dir):
    """The model's context length from its config.json, or None."""
    try:
        with open(os.path.join(model_dir, "config.json"), encoding="utf-8") as f:
            cfg = json.load(f)
    except (OSError, ValueError):
        return None
    for src in (cfg, cfg.get("text_config") or {}):
        for key in ("max_position_embeddings", "n_positions", "max_sequence_length", "seq_length"):
            v = src.get(key)
            if isinstance(v, int) and v > 0:
                return v
    return None


class FakeBackend:
    """Deterministic stand-in used by tests: no torch needed."""

    name = "fake-model"
    device = "cpu"
    context_length = 32768

    def __init__(self, parser: Parser):
        self.parser = parser

    def load(self):
        time.sleep(0.05)

    def generate(self, messages, tools, max_tokens, temperature, stop=None) -> Iterator[str]:
        last = next((m.get("content") or "" for m in reversed(messages) if m["role"] == "user"), "")
        low = last.lower()
        names = [t["function"]["name"] for t in (tools or [])]

        def call(name, args):
            return '<tool_call>\n{"name": "%s", "arguments": %s}\n</tool_call>' % (name, json.dumps(args))

        if messages and messages[-1]["role"] == "tool":
            text = "Done: " + (messages[-1].get("content") or "")[:150]
        elif "clear enough to act on" in low and low.rstrip().endswith("most need answered."):
            text = "CLEAR"
        elif "whole goal in one go" in low:
            goal = low.split("the user's goal:")[-1]
            text = "ONE GO" if ("date" in goal or "time" in goal) else "PLAN: it has several parts."
        elif "one step of it:" in low:
            step = low.split("one step of it:")[1].split("\n")[0]
            text = "BREAK DOWN: it has parts." if "summary" in step else "ONE GO"
        elif "tried 5 times" in low:
            text = "Every try was judged incomplete, so it needs more to go on."
        elif low.rstrip().endswith("title for this conversation:"):
            # Chummy naming a chat ("Use model within app").
            text = "A Friendly Greeting"
        elif "still has unchecked steps" in low:
            # The Chummy assistant sends the model back to finish its to-do list.
            text = "- [x] Find the date\n- [x] Tell you\n\nAll done: the date is in the tool's result above."
        elif "current_time" in names and "step by step" in low:
            # The Chummy assistant: a to-do list and a tool call in one answer.
            text = "I'll make a list and look up the date.\n\n- [ ] Find the date\n- [ ] Tell you\n" + call(
                "current_time", {})
        elif "current_time" in names and ("date" in low or "time" in low):
            text = call("current_time", {})
        elif "run_shell" in names and "shell" in low:
            # A real shell command (Container or OpenShell sandboxes only).
            text = call("run_shell", {"command": "echo made by $(id -u) > shell.txt; cat shell.txt; uname -s"})
        elif "calculator" in names and ("work out" in low or "calculate" in low):
            text = call("calculator", {"expression": "365 - 326"})
        elif "write one yes-or-no question" in low:
            text = "Does this answer make sense given the request?"
        elif "yes or no" in low:
            text = "NO, it needs more." if "write a short summary" in low else "YES, it fits."
        elif "better plan" in low or ("too big" in low and "numbered" in low and "get ready" not in low):
            text = "1. Gather what is needed\n2. Finish the smaller part"
        elif "numbered" in low:
            text = "1. Find out today's date\n2. Work out how many days are left in the year\n3. Write a short summary"
        elif names and ("get ready" in low or "write a short summary" in low):
            text = "TOO BIG"
        elif tools and "weather" in low:
            name = tools[0]["function"]["name"]
            text = 'Let me check.\n<tool_call>\n{"name": "%s", "arguments": {"city": "Paris"}}\n</tool_call>' % name
        elif "think" in low:
            text = "<think>Pondering.</think>Done thinking."
        elif "yes or no" in low:
            text = "YES"
        elif "single word: ready" in low:
            text = "ready"
        else:
            # Echo the most specific part of the prompt so loops can be traced.
            said = low.split("the user now says:")[-1].split("write the assistant")[0].strip() or last.strip()
            text = "(fake model) You said: " + said[:200]
        for i in range(0, len(text), 3):
            if stop is not None and stop.is_set():
                return
            yield text[i:i + 3]

    def count(self, text: str) -> int:
        return max(1, len(text) // 4)


class TransformersBackend:
    def __init__(self, model_dir: str, precision: str, device: str):
        self.model_dir = model_dir
        self.precision = precision
        self.want_device = device
        self.name = os.path.basename(os.path.normpath(model_dir))
        self.device = "cpu"
        self.tok = None
        self.model = None
        self.lock = threading.Lock()
        self.context_length = read_context_length(model_dir)

    def load(self):
        import torch
        from transformers import AutoModelForCausalLM, AutoTokenizer

        dev = self.want_device
        if dev == "auto":
            if torch.cuda.is_available():
                dev = "cuda"
            elif getattr(torch.backends, "mps", None) and torch.backends.mps.is_available():
                dev = "mps"
            else:
                dev = "cpu"
        self.device = dev

        kwargs = {"trust_remote_code": False, "use_safetensors": True, "low_cpu_mem_usage": True}
        if self.precision in ("int8", "int4"):
            if dev != "cuda":
                raise RuntimeError("8-bit and 4-bit loading need an NVIDIA GPU (bitsandbytes).")
            from transformers import BitsAndBytesConfig

            kwargs["quantization_config"] = (
                BitsAndBytesConfig(load_in_8bit=True)
                if self.precision == "int8"
                else BitsAndBytesConfig(load_in_4bit=True, bnb_4bit_quant_type="nf4",
                                        bnb_4bit_compute_dtype=torch.bfloat16)
            )
            kwargs["device_map"] = "auto"
        else:
            if dev == "cuda":
                half = torch.bfloat16 if torch.cuda.is_bf16_supported() else torch.float16
            elif dev == "mps":
                half = torch.float16
            else:
                half = torch.bfloat16
            kwargs["dtype"] = half
            if dev == "cuda":
                kwargs["device_map"] = "auto"

        self.tok = AutoTokenizer.from_pretrained(self.model_dir, trust_remote_code=False)
        try:
            self.model = AutoModelForCausalLM.from_pretrained(self.model_dir, **kwargs)
        except TypeError:
            # transformers < 4.56 calls it torch_dtype
            if "dtype" in kwargs:
                kwargs["torch_dtype"] = kwargs.pop("dtype")
            self.model = AutoModelForCausalLM.from_pretrained(self.model_dir, **kwargs)
        if dev in ("mps", "cpu") and "device_map" not in kwargs:
            self.model.to(dev)
        self.model.eval()

    def _keep_tokens(self, parser: Parser) -> set[str]:
        return set(parser.markers) | {"<think>", "</think>", "<|/tool_call|>", "[ARGS]", "</tool_call>"}

    def generate(self, messages, tools, max_tokens, temperature, parser: Parser,
                 stop: threading.Event | None = None) -> Iterator[str]:
        import torch
        from transformers import StoppingCriteria, StoppingCriteriaList, TextIteratorStreamer

        stop = stop or threading.Event()

        class StopWhenAsked(StoppingCriteria):
            def __call__(self, input_ids, scores, **kwargs):
                return stop.is_set()

        with self.lock:
            enc = self.tok.apply_chat_template(
                messages,
                tools=tools or None,
                add_generation_prompt=True,
                return_tensors="pt",
                return_dict=True,
            )
            enc = {k: v.to(self.model.device) for k, v in enc.items()}
            streamer = TextIteratorStreamer(self.tok, skip_prompt=True, skip_special_tokens=False)
            gen = dict(enc, streamer=streamer, max_new_tokens=max_tokens,
                       stopping_criteria=StoppingCriteriaList([StopWhenAsked()]))
            # Some templates (DeepSeek-R1 distills) open <think> in the prompt,
            # so the answer starts inside it. Say so, so it's routed as reasoning.
            tail = self.tok.decode(enc["input_ids"][0][-8:], skip_special_tokens=False)
            prompt_opens_think = tail.rstrip().endswith("<think>")
            if temperature and temperature > 0:
                gen.update(do_sample=True, temperature=float(temperature))
            else:
                gen.update(do_sample=False)
            if self.tok.pad_token_id is None:
                gen["pad_token_id"] = self.tok.eos_token_id

            drop = [t for t in self.tok.all_special_tokens if t not in self._keep_tokens(parser)]
            err = []

            def run():
                try:
                    with torch.inference_mode():
                        self.model.generate(**gen)
                except Exception as e:  # surfaced after the stream ends
                    err.append(e)
                    streamer.end()

            th = threading.Thread(target=run, daemon=True)
            th.start()
            try:
                if prompt_opens_think:
                    yield "<think>"
                for piece in streamer:
                    for t in drop:
                        if t:
                            piece = piece.replace(t, "")
                    if piece:
                        yield piece
            finally:
                # The client went away or the stream ended: stop generating
                # now, rather than holding the model until max_tokens.
                stop.set()
                th.join()
            if err:
                raise err[0]

    def count(self, text: str) -> int:
        return len(self.tok.encode(text, add_special_tokens=False)) if self.tok else 0


# --------------------------------------------------------------------------
# OpenAI message conversion
# --------------------------------------------------------------------------


def to_hf_messages(messages: list[dict]) -> list[dict]:
    out = []
    for m in messages:
        role = m.get("role")
        content = m.get("content")
        if isinstance(content, list):  # content parts → plain text
            content = "".join(p.get("text", "") for p in content if isinstance(p, dict))
        msg = {"role": role, "content": content or ""}
        if role == "assistant" and m.get("tool_calls"):
            calls = []
            for c in m["tool_calls"]:
                fn = c.get("function", {})
                args = fn.get("arguments", {})
                if isinstance(args, str):
                    args = _json_or_raw(args) or {}
                calls.append({"id": c.get("id"), "type": "function",
                              "function": {"name": fn.get("name"), "arguments": args}})
            msg["tool_calls"] = calls
        if role == "tool":
            if m.get("tool_call_id"):
                msg["tool_call_id"] = m["tool_call_id"]
            if m.get("name"):
                msg["name"] = m["name"]
        out.append(msg)
    return out


# --------------------------------------------------------------------------
# HTTP server
# --------------------------------------------------------------------------


MAX_REQUEST_BYTES = 32 * 1024 * 1024


class State:
    def __init__(self, backend, parser: Parser, token: str):
        self.backend = backend
        self.parser = parser
        self.token = token
        self.status = "loading"
        self.error = None
        self.gen_lock = threading.Lock()


def make_handler(state: State):
    class Handler(BaseHTTPRequestHandler):
        server_version = "ChummyEngine/1"

        def log_message(self, fmt, *args):  # quiet
            pass

        def _auth(self) -> bool:
            given = self.headers.get("Authorization", "").encode()
            if hmac.compare_digest(given, f"Bearer {state.token}".encode()):
                return True
            self._json(401, {"error": {"message": "missing or wrong token"}})
            return False

        def _json(self, code, obj):
            body = json.dumps(obj).encode()
            self.send_response(code)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            if not self._auth():
                return
            if self.path == "/health":
                self._json(200, {"status": state.status, "error": state.error,
                                 "model": state.backend.name, "device": state.backend.device,
                                 "parser": state.parser.name})
            elif self.path == "/v1/models":
                card = {"id": state.backend.name, "object": "model"}
                if getattr(state.backend, "context_length", None):
                    # Not in OpenAI's own API; vLLM, SGLang and others report it like this.
                    card["max_model_len"] = state.backend.context_length
                self._json(200, {"object": "list", "data": [card]})
            else:
                self._json(404, {"error": {"message": "not found"}})

        def do_POST(self):
            if not self._auth():
                return
            if self.path != "/v1/chat/completions":
                return self._json(404, {"error": {"message": "not found"}})
            if state.status != "ready":
                return self._json(503, {"error": {"message": f"model is {state.status}: {state.error or ''}"}})
            try:
                n = int(self.headers.get("Content-Length", "0"))
            except ValueError:
                n = -1
            if not 0 <= n <= MAX_REQUEST_BYTES:
                return self._json(413, {"error": {"message": "request too large"}})
            try:
                req = json.loads(self.rfile.read(n) or b"{}")
                if not isinstance(req, dict):
                    raise ValueError("not an object")
            except ValueError:
                return self._json(400, {"error": {"message": "invalid JSON"}})
            with state.gen_lock:  # one answer at a time
                self._complete(req)

        def _complete(self, req: dict):
            try:
                messages = to_hf_messages(req.get("messages") or [])
                tools = req.get("tools") or []
                max_tokens = int(req.get("max_tokens") or req.get("max_completion_tokens") or 2048)
                max_tokens = max(1, min(max_tokens, 131072))
                temperature = req.get("temperature")
                temperature = float(temperature) if temperature is not None else None
            except (TypeError, ValueError, AttributeError) as e:
                return self._json(400, {"error": {"message": f"bad request: {e}"}})
            stream = bool(req.get("stream"))
            cid = "chatcmpl-" + uuid.uuid4().hex[:24]
            created = int(time.time())
            model = state.backend.name

            def chunk(delta, finish=None, usage=None):
                obj = {"id": cid, "object": "chat.completion.chunk", "created": created, "model": model,
                       "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}
                if usage is not None:
                    obj["choices"] = []
                    obj["usage"] = usage
                return obj

            splitter = StreamSplitter(state.parser)
            text_parts, reasoning_parts, produced = [], [], []

            stop = threading.Event()

            def pieces():
                if isinstance(state.backend, FakeBackend):
                    yield from state.backend.generate(messages, tools, max_tokens, temperature, stop)
                else:
                    yield from state.backend.generate(messages, tools, max_tokens, temperature, state.parser,
                                                      stop)

            if stream:
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.send_header("Cache-Control", "no-cache")
                self.send_header("Connection", "close")
                self.end_headers()

            def send(obj):
                self.wfile.write(b"data: " + json.dumps(obj, ensure_ascii=False).encode() + b"\n\n")
                self.wfile.flush()

            gen = pieces()
            try:
                for piece in gen:
                    produced.append(piece)
                    for kind, s in splitter.feed(piece):
                        if kind == "text":
                            text_parts.append(s)
                            if stream:
                                send(chunk({"content": s}))
                        else:
                            reasoning_parts.append(s)
                            if stream:
                                send(chunk({"reasoning_content": s}))
                tail, calls = splitter.finish()
                for kind, s in tail:
                    (text_parts if kind == "text" else reasoning_parts).append(s)
                    if stream:
                        send(chunk({"content": s} if kind == "text" else {"reasoning_content": s}))
            except (BrokenPipeError, ConnectionResetError):
                # Chummy stopped the run and hung up.
                return
            except Exception as e:  # noqa: BLE001
                try:
                    if stream:
                        send({"error": {"message": f"generation failed: {e}"}})
                        return
                    return self._json(500, {"error": {"message": f"generation failed: {e}"}})
                except OSError:
                    return
            finally:
                stop.set()
                gen.close()

            tool_calls = [{"index": i, "id": "call_" + uuid.uuid4().hex[:16], "type": "function",
                           "function": {"name": c["name"], "arguments": c["arguments"]}}
                          for i, c in enumerate(calls)]
            out_text = "".join(produced)
            usage = {"prompt_tokens": 0, "completion_tokens": state.backend.count(out_text)}
            usage["total_tokens"] = usage["completion_tokens"]
            finish = "tool_calls" if tool_calls else "stop"
            if len(produced) and usage["completion_tokens"] >= max_tokens:
                finish = "length" if not tool_calls else finish

            if stream:
                if tool_calls:
                    send(chunk({"tool_calls": tool_calls}))
                send(chunk({}, finish))
                send(chunk({}, usage=usage))
                self.wfile.write(b"data: [DONE]\n\n")
                self.wfile.flush()
            else:
                msg = {"role": "assistant", "content": "".join(text_parts)}
                if reasoning_parts:
                    msg["reasoning_content"] = "".join(reasoning_parts)
                if tool_calls:
                    msg["tool_calls"] = [{k: v for k, v in c.items() if k != "index"} for c in tool_calls]
                self._json(200, {"id": cid, "object": "chat.completion", "created": created, "model": model,
                                 "choices": [{"index": 0, "message": msg, "finish_reason": finish}],
                                 "usage": usage})

    return Handler


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--model", help="folder with config.json, tokenizer and *.safetensors")
    ap.add_argument("--parser", default="hermes", choices=sorted(PARSERS))
    ap.add_argument("--port", type=int, required=True, help="0 picks a free port")
    ap.add_argument("--precision", default="half", choices=["half", "int8", "int4"])
    ap.add_argument("--device", default="auto", choices=["auto", "cuda", "mps", "cpu"])
    ap.add_argument("--fake", action="store_true", help="use a canned model (tests)")
    a = ap.parse_args(argv)

    token = os.environ.get("CHUMMY_ENGINE_TOKEN", "")
    if len(token) < 16:
        print("CHUMMY_ENGINE_TOKEN must be set", file=sys.stderr)
        return 2

    parser = PARSERS[a.parser]
    backend = FakeBackend(parser) if a.fake else TransformersBackend(a.model, a.precision, a.device)
    state = State(backend, parser, token)

    def load():
        try:
            print(f"loading {backend.name}...", flush=True)
            backend.load()
            state.status = "ready"
            print(f"ready on {backend.device}", flush=True)
        except Exception as e:  # noqa: BLE001
            state.status = "error"
            state.error = f"{type(e).__name__}: {e}"
            print(f"failed to load: {state.error}", file=sys.stderr, flush=True)

    threading.Thread(target=load, daemon=True).start()
    # Port 0 lets the OS pick a free port, which is then reported here. The
    # app reads it from this line, so no other program can take the port in
    # between (and receive the token).
    httpd = ThreadingHTTPServer(("127.0.0.1", a.port), make_handler(state))
    httpd.daemon_threads = True
    print(f"listening on 127.0.0.1:{httpd.server_address[1]}", flush=True)
    try:
        httpd.serve_forever(poll_interval=0.2)
    except KeyboardInterrupt:
        pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
