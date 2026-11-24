"""Tests for the built-in engine that need only the standard library.

Run: python3 -m unittest discover -s crates/local/engine
"""

import json
import os
import socket
import subprocess
import sys
import time
import unittest
import urllib.request

sys.path.insert(0, os.path.dirname(__file__))
import chummy_engine as ce  # noqa: E402


def args(call):
    return json.loads(call["arguments"])


class ParserTests(unittest.TestCase):
    def test_hermes(self):
        text, calls = ce.PARSERS["hermes"].parse(
            'Sure.\n<tool_call>\n{"name": "a", "arguments": {"x": 1}}\n</tool_call>\n'
            '<tool_call>{"name": "b", "arguments": {}}</tool_call>'
        )
        self.assertEqual(text, "Sure.")
        self.assertEqual([c["name"] for c in calls], ["a", "b"])
        self.assertEqual(args(calls[0]), {"x": 1})

    def test_hermes_malformed_and_unclosed(self):
        _, calls = ce.PARSERS["hermes"].parse('<tool_call>{"name": "a", "arguments": {x: 1}}')
        self.assertEqual(calls[0]["name"], "a")
        self.assertIn("{x: 1}", calls[0]["arguments"])

    def test_mistral_both_formats(self):
        _, calls = ce.PARSERS["mistral"].parse('[TOOL_CALLS][{"name": "a", "arguments": {"q": "z"}}]')
        self.assertEqual((calls[0]["name"], args(calls[0])), ("a", {"q": "z"}))
        _, calls = ce.PARSERS["mistral"].parse('[TOOL_CALLS]get[ARGS]{"k": 2}[TOOL_CALLS]put[ARGS]{}')
        self.assertEqual([c["name"] for c in calls], ["get", "put"])
        self.assertEqual(args(calls[0]), {"k": 2})

    def test_llama3(self):
        _, calls = ce.PARSERS["llama3_json"].parse(
            '<|python_tag|>{"name": "a", "parameters": {"s": "x;y"}}; {"name": "b", "parameters": {}}'
        )
        self.assertEqual([c["name"] for c in calls], ["a", "b"])
        self.assertEqual(args(calls[0]), {"s": "x;y"})
        text, calls = ce.PARSERS["llama3_json"].parse("Just words.")
        self.assertEqual((text, calls), ("Just words.", []))

    def test_granite(self):
        _, calls = ce.PARSERS["granite"].parse('<|tool_call|>[{"name": "a", "arguments": {"n": 3}}]<|/tool_call|>')
        self.assertEqual(args(calls[0]), {"n": 3})


class SplitterTests(unittest.TestCase):
    def run_pieces(self, parser, text, size=2):
        sp = ce.StreamSplitter(ce.PARSERS[parser])
        out = []
        for i in range(0, len(text), size):
            out += sp.feed(text[i:i + size])
        tail, calls = sp.finish()
        out += tail
        joined = {"text": "", "reasoning": ""}
        for k, s in out:
            joined[k] += s
        return joined, calls

    def test_marker_never_leaks(self):
        j, calls = self.run_pieces("hermes", 'Hi there.\n<tool_call>\n{"name": "f", "arguments": {}}\n</tool_call>')
        self.assertEqual(j["text"].strip(), "Hi there.")
        self.assertNotIn("<", j["text"])
        self.assertEqual(calls[0]["name"], "f")

    def test_plain_text_passes_through(self):
        j, calls = self.run_pieces("hermes", "a < b and c <tool not a call", size=3)
        self.assertEqual(j["text"], "a < b and c <tool not a call")
        self.assertEqual(calls, [])

    def test_think_becomes_reasoning(self):
        j, calls = self.run_pieces("hermes", "<think>\nhmm, ok\n</think>\n\nAnswer.")
        self.assertEqual(j["reasoning"].strip(), "hmm, ok")
        self.assertEqual(j["text"].strip(), "Answer.")

    def test_llama_bare_json(self):
        j, calls = self.run_pieces("llama3_json", '{"name": "f", "parameters": {"a": 1}}')
        self.assertEqual(j["text"], "")
        self.assertEqual(args(calls[0]), {"a": 1})

    def test_json_answer_that_is_not_a_call_is_kept(self):
        # Llama treats a leading "{" as a possible call; if it isn't one,
        # the text must still reach the user.
        j, calls = self.run_pieces("llama3_json", '{"total": 3, "unit": "apples"}')
        self.assertEqual(calls, [])
        self.assertEqual(j["text"], '{"total": 3, "unit": "apples"}')

    def test_prompt_opened_think(self):
        # The backend sends "<think>" first when the prompt opened it.
        sp = ce.StreamSplitter(ce.PARSERS["hermes"])
        out = sp.feed("<think>") + sp.feed("plan</th") + sp.feed("ink>\nAnswer")
        tail, _ = sp.finish()
        joined = {"text": "", "reasoning": ""}
        for k, v in out + tail:
            joined[k] += v
        self.assertEqual(joined, {"reasoning": "plan", "text": "Answer"})

    def test_fake_backend_stops_when_asked(self):
        import threading
        stop = threading.Event()
        gen = ce.FakeBackend(ce.PARSERS["hermes"]).generate(
            [{"role": "user", "content": "x" * 300}], [], 100, None, stop)
        first = next(gen)
        stop.set()
        self.assertTrue(first)
        self.assertEqual(list(gen), [])


    def test_fake_backend_answers_the_assistants_judges(self):
        def say(prompt):
            gen = ce.FakeBackend(ce.PARSERS["hermes"]).generate(
                [{"role": "user", "content": prompt}], [], 100, None, None)
            return "".join(gen)
        self.assertEqual(say("Hi\n\nIs the user's latest request clear enough to act on? If it is, reply with only: CLEAR\n"
                             "If not, reply with the one short question you most need answered."), "CLEAR")
        self.assertEqual(say("What's the date?\nCould you do this whole goal in one go"), "ONE GO")
        self.assertTrue(say("A report\nCould you do this whole goal in one go").startswith("PLAN"))
        self.assertEqual(say("One step of it: Find the date\nCould you do this step in one go"), "ONE GO")
        self.assertTrue(say("One step of it: Write a short summary\nOne go?").startswith("BREAK DOWN"))

    def test_fake_backend_plays_the_agent_loop_assistant(self):
        tools = [{"type": "function", "function": {"name": "current_time", "parameters": {}}}]
        def say(prompt):
            gen = ce.FakeBackend(ce.PARSERS["hermes"]).generate(
                [{"role": "user", "content": prompt}], tools, 100, None, None)
            return "".join(gen)
        first = say("Find the date, step by step")
        self.assertIn("- [ ] Find the date", first)
        self.assertIn('"name": "current_time"', first)
        self.assertTrue(say("But your to-do list still has unchecked steps:\n- [ ] Tell you").startswith("- [x]"))
        self.assertEqual(say("The user wrote:\nhi\n\nTitle for this conversation:"), "A Friendly Greeting")
        self.assertEqual(
            say("You are directing the work on the user's request:\nCount the days\n\nThe to-do list:\n(none yet)\n\n"
                "Decide the ONE next step to take now to finish the user's request."),
            "Work on the request: Count the days")
        # A step reminder sent last doesn't hide a tool result before it.
        gen = ce.FakeBackend(ce.PARSERS["hermes"]).generate(
            [{"role": "user", "content": "What time is it"},
             {"role": "tool", "content": "12:00"},
             {"role": "user", "content": "(A note from your own planning, not from the user.) Your step now: x"}],
            tools, 100, None, None)
        self.assertEqual("".join(gen), "Done: 12:00")


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


class ServerTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.port = free_port()
        cls.token = "t" * 32
        env = dict(os.environ, CHUMMY_ENGINE_TOKEN=cls.token)
        cls.proc = subprocess.Popen(
            [sys.executable, os.path.join(os.path.dirname(__file__), "chummy_engine.py"),
             "--fake", "--port", str(cls.port)],
            env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        for _ in range(100):
            try:
                if cls.get("/health")["status"] == "ready":
                    break
            except OSError:
                pass
            time.sleep(0.05)

    @classmethod
    def tearDownClass(cls):
        cls.proc.terminate()
        cls.proc.wait(5)

    @classmethod
    def req(cls, path, body=None, token=None):
        r = urllib.request.Request(f"http://127.0.0.1:{cls.port}{path}",
                                   data=json.dumps(body).encode() if body is not None else None,
                                   headers={"Authorization": f"Bearer {token or cls.token}",
                                            "Content-Type": "application/json"})
        return urllib.request.urlopen(r, timeout=5)

    @classmethod
    def get(cls, path):
        return json.loads(cls.req(path).read())

    def test_models_report_the_context_length(self):
        card = self.get("/v1/models")["data"][0]
        self.assertEqual(card["max_model_len"], 32768)

    def test_context_length_from_config(self):
        import tempfile
        with tempfile.TemporaryDirectory() as d:
            self.assertIsNone(ce.read_context_length(d))
            with open(os.path.join(d, "config.json"), "w") as f:
                json.dump({"text_config": {"max_position_embeddings": 40960}}, f)
            self.assertEqual(ce.read_context_length(d), 40960)

    def test_requires_token(self):
        with self.assertRaises(urllib.error.HTTPError) as e:
            self.req("/health", token="wrong-token-wrong-token")
        self.assertEqual(e.exception.code, 401)

    def test_rejects_bad_requests(self):
        for body, code in [([1, 2], 400), ({"messages": [], "max_tokens": "lots"}, 400)]:
            with self.assertRaises(urllib.error.HTTPError) as e:
                self.req("/v1/chat/completions", body)
            self.assertEqual(e.exception.code, code)
        # The server still works afterwards.
        self.assertEqual(self.get("/health")["status"], "ready")

    def test_hanging_up_mid_stream_frees_the_engine(self):
        body = {"messages": [{"role": "user", "content": "x" * 2000}], "stream": True}
        r = self.req("/v1/chat/completions", body)
        r.read(10)
        r.close()  # hang up
        # The next request must not wait for the abandoned one.
        t = time.time()
        body = {"messages": [{"role": "user", "content": "Reply with the single word: ready"}]}
        out = json.loads(self.req("/v1/chat/completions", body).read())
        self.assertEqual(out["choices"][0]["message"]["content"], "ready")
        self.assertLess(time.time() - t, 3)

    def test_non_streamed_tool_call(self):
        body = {"messages": [{"role": "user", "content": "weather in Paris?"}],
                "tools": [{"type": "function", "function": {"name": "get_weather", "parameters": {}}}]}
        r = json.loads(self.req("/v1/chat/completions", body).read())
        msg = r["choices"][0]["message"]
        self.assertEqual(msg["content"].strip(), "Let me check.")
        self.assertEqual(msg["tool_calls"][0]["function"]["name"], "get_weather")
        self.assertEqual(r["choices"][0]["finish_reason"], "tool_calls")

    def test_streamed_text(self):
        body = {"messages": [{"role": "user", "content": "Reply with the single word: ready"}], "stream": True}
        raw = self.req("/v1/chat/completions", body).read().decode()
        events = [l[6:] for l in raw.splitlines() if l.startswith("data: ")]
        self.assertEqual(events[-1], "[DONE]")
        text = "".join(json.loads(e)["choices"][0]["delta"].get("content", "")
                       for e in events[:-1] if json.loads(e).get("choices"))
        self.assertEqual(text, "ready")


if __name__ == "__main__":
    unittest.main()
