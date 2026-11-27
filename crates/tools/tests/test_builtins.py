"""Tests for the built-in tools' logic, run with an ordinary Python:

    python3 -m unittest discover -s crates/tools/tests -p 'test_*.py'

The real sandbox (WebAssembly, /workspace) is tested from Rust in
sandbox.rs; here the `chummy` helper module's workspace is pointed at a
temporary folder, so the tools' behaviour can be checked quickly.
"""

import importlib.util
import os
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SUPPORT = os.path.join(HERE, "..", "support")
BUILTIN = os.path.join(HERE, "..", "builtin")
sys.path.insert(0, SUPPORT)
import chummy  # noqa: E402


def load(name):
    spec = importlib.util.spec_from_file_location(f"tool_{name}", os.path.join(BUILTIN, f"{name}.py"))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return getattr(mod, name)


class Workspace(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.ws = os.path.realpath(self.tmp.name)
        self.old = chummy.WORKSPACE, os.getcwd()
        chummy.WORKSPACE = self.ws
        os.chdir(self.ws)

    def tearDown(self):
        chummy.WORKSPACE, cwd = self.old
        os.chdir(cwd)
        self.tmp.cleanup()

    def put(self, path, text):
        full = os.path.join(self.ws, path)
        os.makedirs(os.path.dirname(full), exist_ok=True)
        with open(full, "w", encoding="utf-8", newline="") as f:
            f.write(text)

    def get(self, path):
        with open(os.path.join(self.ws, path), encoding="utf-8", newline="") as f:
            return f.read()


class ReadFile(Workspace):
    def test_chunks_with_line_numbers(self):
        read = load("read_file")
        self.put("big.txt", "".join(f"line {i}\n" for i in range(1, 501)))
        r = read("big.txt", max_lines=3)
        self.assertEqual(
            r,
            "big.txt: lines 1–3 of 500\n     1\tline 1\n     2\tline 2\n     3\tline 3\n"
            "[Lines 4–500 not shown. To read on, use line_number=4.]",
        )
        r = read("big.txt", line_number=499, max_lines=10)
        self.assertTrue(r.startswith("big.txt: lines 499–500 of 500"), r)
        self.assertNotIn("not shown", r)
        with self.assertRaises(chummy.ToolError):
            read("big.txt", line_number=600)

    def test_long_output_stops_early_and_says_where(self):
        read = load("read_file")
        self.put("wide.txt", ("x" * 1900 + "\n") * 100)
        r = read("wide.txt", max_lines=5000)
        self.assertLessEqual(len(r), 41_000)
        self.assertIn("To read on, use line_number=", r)

    def test_edge_cases(self):
        read = load("read_file")
        self.put("empty.txt", "")
        self.assertEqual(read("empty.txt"), "empty.txt is empty.")
        with open(os.path.join(self.ws, "b.bin"), "wb") as f:
            f.write(b"\0\1\2")
        with self.assertRaises(chummy.ToolError):
            read("b.bin")
        with self.assertRaises(chummy.ToolError):
            read("missing.txt")


class EditFile(Workspace):
    def setUp(self):
        super().setUp()
        self.edit = load("edit_file")
        self.put("app.py", "def a():\n    return 1\n\ndef b():\n    return 1\n")

    def test_replaces_one_exact_match(self):
        r = self.edit("app.py", "def b():\n    return 1", "def b():\n    return 2")
        self.assertEqual(self.get("app.py"), "def a():\n    return 1\n\ndef b():\n    return 2\n")
        self.assertIn("Edited app.py: changed 1 place. Lines 4–5", r)
        self.assertIn("     5\t    return 2", r)

    def test_ambiguous_and_missing_matches_are_refused(self):
        with self.assertRaises(chummy.ToolError) as e:
            self.edit("app.py", "    return 1", "    return 3")
        self.assertIn("matches 2 places", str(e.exception))
        self.assertIn("lines 2, 5", str(e.exception))
        self.edit("app.py", "    return 1", "    return 3", replace_all=True)
        self.assertEqual(self.get("app.py").count("return 3"), 2)
        with self.assertRaises(chummy.ToolError) as e:
            self.edit("app.py", "def c():", "def d():")
        self.assertIn("doesn't match", str(e.exception))
        with self.assertRaises(chummy.ToolError):
            self.edit("app.py", "", "x")
        with self.assertRaises(chummy.ToolError):
            self.edit("nope.py", "a", "b")

    def test_forgives_line_numbers_and_trailing_spaces(self):
        self.edit("app.py", "     4\tdef b():\n     5\t    return 1", "     4\tdef b():\n     5\t    return 9")
        self.assertIn("    return 9", self.get("app.py"))
        self.put("t.txt", "alpha   \nbeta\n")
        self.edit("t.txt", "alpha\nbeta", "gamma\nbeta")
        self.assertEqual(self.get("t.txt"), "gamma\nbeta\n")

    def test_keeps_windows_line_endings(self):
        self.put("w.txt", "one\r\ntwo\r\n")
        self.edit("w.txt", "one\ntwo", "one\nthree")
        self.assertEqual(self.get("w.txt"), "one\r\nthree\r\n")


class WriteSearchFind(Workspace):
    def test_write_reports(self):
        write = load("write_file")
        self.assertEqual(write("a/b.txt", "1\n2\n"), "Created a/b.txt: 2 lines.")
        self.assertEqual(write("a/b.txt", "1\n"), "Replaced a/b.txt: 1 line (it had 2).")
        self.assertIn("Added 1 line", write("a/b.txt", "2\n", append=True))

    def test_search(self):
        search = load("search_files")
        self.put("src/a.py", "x = 1\n# TODO fix\ny = 2\n")
        self.put("src/b.md", "todo: nothing\n")
        self.put("node_modules/x.js", "TODO hidden\n")
        r = search("todo")
        self.assertIn("src/a.py:2: # TODO fix", r)
        self.assertIn("src/b.md:1: todo: nothing", r)
        self.assertNotIn("node_modules", r)
        r = search("TODO", ignore_case=False, file_glob="*.py", context_lines=1)
        self.assertEqual(r, "src/a.py-1- x = 1\nsrc/a.py:2: # TODO fix\nsrc/a.py-3- y = 2")
        self.assertEqual(search(r"y\s*=\s*\d", regex=True), "src/a.py:3: y = 2")
        with self.assertRaises(chummy.ToolError):
            search("(", regex=True)

    def test_find(self):
        find = load("find_files")
        self.put("a.py", "")
        self.put("src/deep/b.py", "")
        self.put("src/c.md", "")
        self.assertEqual(sorted(find("*.py").split("\n")), ["a.py", "src/deep/b.py"])
        self.assertEqual(find("src/*.md"), "src/c.md")
        self.assertIn("No files match", find("*.rs"))


class RunCommand(Workspace):
    def setUp(self):
        super().setUp()
        self.run = load("run_command")

    def test_pipes_redirects_and_lists(self):
        self.put("names.txt", "bob\nalice\nbob\ncarol\n")
        self.assertEqual(self.run("sort names.txt | uniq | head -n 2"), "alice\nbob\n[exit code 0]")
        self.assertEqual(self.run("grep -c bob names.txt"), "2\n[exit code 0]")
        self.run("mkdir -p out && echo hi > out/a.txt && echo more >> out/a.txt")
        self.assertEqual(self.get("out/a.txt"), "hi\nmore\n")
        r = self.run("cat nope.txt || echo fallback")
        self.assertIn("fallback", r)
        self.assertIn("no such file", r)
        self.assertTrue(r.endswith("[exit code 0]"))
        self.assertEqual(self.run("false && echo no; echo yes"), "yes\n[exit code 0]")
        self.assertEqual(self.run("cat nope.txt 2>/dev/null"), "(no output)\n[exit code 1]")
        self.assertEqual(self.run("wc -l < names.txt"), "4\n[exit code 0]")
        self.assertEqual(self.run("echo 'a b' \"c\" | tr a-c A-C"), "A B C\n[exit code 0]")

    def test_files_and_globs(self):
        self.put("a.py", "print('a')\n")
        self.put("b.py", "print('b')\n")
        self.assertEqual(self.run("ls *.py"), "a.py\nb.py\n[exit code 0]")
        self.run("cp a.py c.py && mv b.py d.py && rm a.py")
        self.assertEqual(sorted(os.listdir(self.ws)), ["c.py", "d.py"])
        self.assertIn("refusing", self.run("rm -rf ."))
        self.assertIn("outside the workspace", self.run("cat ../x"))
        self.assertIn("outside the workspace", self.run("ls /etc"))
        self.assertIn("needs a command and", self.run("find . -exec rm"))
        self.assertIn("isn't supported", self.run("find . -prune"))
        self.assertEqual(self.run("find . -name '*.py' -type f"), "./c.py\n./d.py\n[exit code 0]")
        self.assertEqual(self.run("cd sub 2>&1"), "cd: sub: no such folder\n[exit code 1]")

    def test_python_runs_in_the_workspace(self):
        self.put("tool.py", "import sys\nprint('args', sys.argv[1:])\nwith open('data.txt') as f: print(f.read().strip())\nsys.exit(3)\n")
        self.put("data.txt", "hello\n")
        self.assertEqual(self.run("python tool.py x y"), "args ['x', 'y']\nhello\n[exit code 3]")
        self.assertEqual(self.run('python -c "print(6*7)"'), "42\n[exit code 0]")
        r = self.run("python -c \"raise ValueError('boom')\"")
        self.assertIn("ValueError: boom", r)
        self.assertTrue(r.endswith("[exit code 1]"))
        self.put("helper.py", "VALUE = 5\n")
        self.put("main.py", "import helper\nprint(helper.VALUE)\n")
        self.assertEqual(self.run("python main.py"), "5\n[exit code 0]")
        self.assertEqual(self.run("echo 'print(1+1)' | python"), "2\n[exit code 0]")

    def test_unknown_commands_explain_themselves(self):
        r = self.run("npm install")
        self.assertIn("no Node.js", r)
        self.assertTrue(r.endswith("[exit code 127]"))
        r = self.run("frobnicate x")
        self.assertIn("frobnicate: command not found", r)
        self.assertIn("help", r)
        self.assertIn("curl and wget", self.run("ssh host"))
        self.assertIn("can't be installed", self.run("pip install requests"))
        self.assertEqual(self.run('bash -c "echo inner"'), "inner\n[exit code 0]")


class Web(Workspace):
    """curl and wget in run_command, with Chummy's side of chummy.http faked."""

    def setUp(self):
        super().setUp()
        self.run = load("run_command")
        self.calls = []
        self.allowed = True
        self.saved = chummy._request

        def fake(op, **f):
            self.calls.append(f)
            if not self.allowed:
                raise chummy.ToolError("Internet access is off for this tool.")
            import base64
            body = b"\x89PNG\x00\xff" if f["url"].endswith(".png") else f"hi from {f['url']}".encode()
            status = 404 if "missing" in f["url"] else 200
            return {"ok": True, "status": status, "url": f["url"], "headers": {"content-type": "text/plain"},
                    "body_b64": base64.b64encode(b"" if f["method"] == "HEAD" else body).decode(), "truncated": False}

        chummy._request = fake

    def tearDown(self):
        chummy._request = self.saved
        super().tearDown()

    def test_curl(self):
        self.assertEqual(self.run("curl -s https://example.com/a"), "hi from https://example.com/a\n[exit code 0]")
        self.assertEqual(self.calls[-1]["method"], "GET")
        self.assertTrue(self.calls[-1]["binary"])
        # Bytes are kept when saved; -O names the file after the URL.
        self.run("curl -sLo pic.png https://example.com/x.png")
        with open(os.path.join(self.ws, "pic.png"), "rb") as f:
            self.assertEqual(f.read(), b"\x89PNG\x00\xff")
        self.run("curl -O https://example.com/dir/y.png")
        self.assertTrue(os.path.exists(os.path.join(self.ws, "y.png")))
        # Data makes a POST, from text, a file or stdin; --json sets the type.
        self.put("body.json", '{"a": 1}')
        self.run("curl -s -H 'X-Key: k' --json @body.json https://example.com/api")
        c = self.calls[-1]
        self.assertEqual((c["method"], c["headers"]["X-Key"], c["headers"]["Content-Type"]),
                         ("POST", "k", "application/json"))
        import base64
        self.assertEqual(base64.b64decode(c["body_b64"]), b'{"a": 1}')
        self.run("curl -s -d a=1 -d b=2 https://example.com/f")
        self.assertEqual(base64.b64decode(self.calls[-1]["body_b64"]), b"a=1&b=2")
        self.run("curl -s -G -d q=x https://example.com/s")
        self.assertEqual((self.calls[-1]["method"], self.calls[-1]["url"]), ("GET", "https://example.com/s?q=x"))
        r = self.run("curl -sI https://example.com/")
        self.assertIn("HTTP/1.1 200", r)
        self.assertEqual(self.calls[-1]["method"], "HEAD")
        self.assertEqual(self.run("curl -s -o /dev/null -w '%{http_code}' https://example.com/"), "200\n[exit code 0]")
        # -f fails on errors; pipes take curl's output.
        self.assertIn("[exit code 22]", self.run("curl -sSf https://example.com/missing"))
        self.assertEqual(self.run("curl -s example.com/b | wc -c"), "28\n[exit code 0]")
        self.assertEqual(self.calls[-1]["url"], "http://example.com/b")
        # What isn't supported says so.
        self.assertIn("multipart", self.run("curl -F a=@x https://example.com"))
        self.assertIn("certificates", self.run("curl -k https://example.com"))
        self.assertIn("only GET, POST and HEAD", self.run("curl -X DELETE https://example.com"))

    def test_wget(self):
        r = self.run("wget https://example.com/files/notes.txt")
        self.assertIn("saved notes.txt", r)
        self.assertEqual(self.get("notes.txt"), "hi from https://example.com/files/notes.txt")
        self.run("wget -q -O out.txt https://example.com/")
        self.assertEqual(self.get("out.txt"), "hi from https://example.com/")
        self.run("mkdir dl")
        self.run("wget -q -P dl https://example.com/")
        self.assertEqual(self.get("dl/index.html"), "hi from https://example.com/")
        self.assertEqual(self.run("wget -qO- https://example.com/"), "hi from https://example.com/\n[exit code 0]")
        self.assertIn("[exit code 8]", self.run("wget https://example.com/missing"))

    def test_internet_off_is_explained(self):
        self.allowed = False
        r = self.run("curl https://example.com")
        self.assertIn("Internet access is off for this tool.", r)
        self.assertTrue(r.endswith("[exit code 7]"))
        self.assertIn("Internet access is off", self.run("wget https://example.com"))
        # Paths still can't leave the workspace.
        self.allowed = True
        self.assertIn("outside the workspace", self.run("curl -o /etc/x https://example.com"))


@unittest.skipIf(os.name == "nt", "needs sh")
class RunShell(Workspace):
    def setUp(self):
        super().setUp()
        self.run = load("run_shell")

    def test_runs_in_the_workspace_with_exit_codes(self):
        self.put("a.txt", "hello\n")
        self.assertEqual(self.run("cat a.txt; pwd"), f"hello\n{self.ws}\n[exit code 0]")

    def test_says_why_there_is_no_network(self):
        failing = "echo 'curl: (6) Could not resolve host: example.com'; exit 6"
        saved = os.environ.get("CHUMMY_NETWORK")
        try:
            for how, want in (("off", "internet access is off"), ("web", "only the Container sandbox")):
                os.environ["CHUMMY_NETWORK"] = how
                self.assertIn(want, self.run(failing))
            os.environ["CHUMMY_NETWORK"] = "direct"
            self.assertTrue(self.run(failing).endswith("[exit code 6]"))
            # Other failures get no hint.
            os.environ["CHUMMY_NETWORK"] = "off"
            self.assertTrue(self.run("exit 3").endswith("[exit code 3]"))
        finally:
            if saved is None:
                os.environ.pop("CHUMMY_NETWORK", None)
            else:
                os.environ["CHUMMY_NETWORK"] = saved
        self.assertEqual(self.run("exit 3"), "(no output)\n[exit code 3]")
        self.assertIn("no-such-program", self.run("no-such-program 2>&1"))

    def test_gets_no_host_environment(self):
        os.environ["CHUMMY_TEST_SECRET"] = "x"
        try:
            self.assertNotIn("CHUMMY_TEST_SECRET", self.run("env"))
        finally:
            del os.environ["CHUMMY_TEST_SECRET"]

    def test_long_output_is_cut_and_empty_commands_refused(self):
        out = self.run("i=0; while [ $i -lt 8000 ]; do echo line $i; i=$((i+1)); done")
        self.assertIn("characters cut", out)
        self.assertTrue(out.endswith("[exit code 0]"))
        with self.assertRaises(chummy.ToolError):
            self.run("  ")


class ShellSyntax(Workspace):
    def setUp(self):
        super().setUp()
        self.run = load("run_command")

    def out(self, cmd):
        r = self.run(cmd)
        text, _, code = r.rpartition("\n")
        return text, code

    def test_several_lines_heredocs_and_continuations(self):
        r = self.run("mkdir -p src\ncat > src/a.txt <<'EOF'\nline $one\n  two\nEOF\ncat src/a.txt | wc -l")
        self.assertEqual(r, "2\n[exit code 0]")
        self.assertEqual(self.get("src/a.txt"), "line $one\n  two\n")
        self.assertEqual(self.run("cat <<-END\n\tindented\n\tEND"), "indented\n[exit code 0]")
        self.assertEqual(self.run("echo one \\\n  two"), "one two\n[exit code 0]")
        self.assertEqual(self.run("echo a |\n  tr a b"), "b\n[exit code 0]")
        self.assertEqual(self.run("# a comment\necho hi # another\n"), "hi\n[exit code 0]")
        self.assertEqual(self.run("tr a-z A-Z <<< hello"), "HELLO\n[exit code 0]")
        self.assertEqual(self.run("python <<'EOF'\nprint(sum(range(5)))\nEOF"), "10\n[exit code 0]")
        self.assertEqual(self.run("cat <<EOF\nno end"), "no end\n[exit code 0]")
        self.assertIn("has no end", self.run("cat <<EOF"))

    def test_quotes_braces_and_redirects(self):
        self.assertEqual(self.run("echo \"a \\\"b\\\" c\" 'd $x'"), 'a "b" c d $x\n[exit code 0]')
        self.run("mkdir -p p/{a,b,c} && touch p/a/f{1..3}.txt")
        self.assertEqual(self.run("ls p p/a"), "p:\na/\nb/\nc/\np/a:\nf1.txt\nf2.txt\nf3.txt\n[exit code 0]")
        self.assertEqual(self.run("echo {x}"), "{x}\n[exit code 0]")
        self.run("cat missing.txt 2> err.txt; echo out > o.txt")
        self.assertIn("no such file", self.get("err.txt"))
        self.assertEqual(self.run("cat missing.txt &> both.txt; cat both.txt"),
                         "cat: missing.txt: no such file\n[exit code 0]")
        self.assertEqual(self.run("echo warn >&2 | wc -c"), "0\nwarn\n[exit code 0]")
        self.assertEqual(self.run("awk '$2>5 {print $1}' <<< 'a 9'"), "a\n[exit code 0]")

    def test_no_variables_or_loops(self):
        for cmd in ("echo $HOME", "echo \"$(ls)\"", "echo `ls`", "for f in *; do echo $f; done",
                    "X=1 echo hi", "export A=b"):
            r = self.run(cmd)
            self.assertIn("no variables", r, cmd)
            self.assertNotIn("exit code 0", r, cmd)
        self.assertIn("subshells", self.run("(cd x && ls)"))
        self.assertIn("<(…)", self.run("diff <(ls) a"))
        self.assertEqual(self.run("echo cost: \\$5"), "cost: $5\n[exit code 0]")

    def test_set_e_exit_and_scripts(self):
        self.assertEqual(self.run("set -e\nfalse\necho never"), "(no output)\n[exit code 1]")
        self.assertEqual(self.run("false || echo ok; echo next"), "ok\nnext\n[exit code 0]")
        self.assertEqual(self.run("echo a; exit 3; echo b"), "a\n[exit code 3]")
        self.assertEqual(self.run("set -o pipefail; false | echo x"), "x\n[exit code 1]")
        self.put("job.sh", "#!/bin/sh\necho from script\nls *.sh\n")
        self.assertEqual(self.run("bash job.sh"), "from script\njob.sh\n[exit code 0]")
        self.assertEqual(self.run("chmod +x job.sh && ./job.sh"), "from script\njob.sh\n[exit code 0]")
        self.put("t.py", "print('py')\n")
        self.assertEqual(self.run("./t.py"), "py\n[exit code 0]")

    def test_help(self):
        r = self.run("help")
        for name in ("sed", "awk", "xargs", "python", "printf"):
            self.assertIn(name, r)
        self.assertIn("s/re/new/", self.run("help sed"))
        self.assertIn("-F sep", self.run("awk --help"))


class Sed(Workspace):
    def setUp(self):
        super().setUp()
        self.run = load("run_command")
        self.put("f.txt", "alpha 1\nbeta 2\n# note\ngamma 3\n\ndelta 4\n")

    def sed(self, args, data=None):
        if data is not None:
            self.put("in.txt", data)
            args += " in.txt"
        return self.run("sed " + args)

    def test_substitute(self):
        self.assertEqual(self.sed("'s/a/A/' f.txt").split("\n")[0], "Alpha 1")
        self.assertEqual(self.sed("'s/a/A/g' f.txt").split("\n")[0], "AlphA 1")
        self.assertEqual(self.sed("'s/a/A/2'", "banana\n"), "banAna\n[exit code 0]")
        self.assertEqual(self.sed("'s/\\(an\\)\\{2\\}/[\\1]/'", "banana\n"), "b[an]a\n[exit code 0]")
        self.assertEqual(self.sed("-E 's/(an)+/<&>/'", "banana\n"), "b<anan>a\n[exit code 0]")
        self.assertEqual(self.sed("'s|/usr|/opt|'", "/usr/bin\n"), "/opt/bin\n[exit code 0]")
        self.assertEqual(self.sed("'s/[[:digit:]]\\+/N/g'", "a1 b22\n"), "aN bN\n[exit code 0]")
        self.assertEqual(self.sed("'s/\\w\\+/\\u&/g'", "hi there\n"), "Hi There\n[exit code 0]")
        self.assertEqual(self.sed("-E 's/(.*)=(.*)/\\U\\2\\E=\\1/'", "k=v\n"), "V=k\n[exit code 0]")
        self.assertEqual(self.sed("'s/x/y/I'", "X\n"), "y\n[exit code 0]")
        self.assertEqual(self.sed("'s/ /\\n/'", "a b\n"), "a\nb\n[exit code 0]")

    def test_addresses_and_commands(self):
        self.assertEqual(self.run("sed -n '2,3p' f.txt"), "beta 2\n# note\n[exit code 0]")
        self.assertEqual(self.run("sed '/^#/d;/^$/d' f.txt | wc -l"), "4\n[exit code 0]")
        self.assertEqual(self.run("sed -n '/beta/,/gamma/p' f.txt"), "beta 2\n# note\ngamma 3\n[exit code 0]")
        self.assertEqual(self.run("sed -n '$p' f.txt"), "delta 4\n[exit code 0]")
        self.assertEqual(self.run("sed '2q' f.txt"), "alpha 1\nbeta 2\n[exit code 0]")
        self.assertEqual(self.run("sed -n '/gamma/=' f.txt"), "4\n[exit code 0]")
        self.assertEqual(self.run("sed '1!d' f.txt"), "alpha 1\n[exit code 0]")
        self.assertEqual(self.run("sed -n '1~2p' f.txt"), "alpha 1\n# note\n[exit code 0]")
        self.assertEqual(self.run("sed -n '2,+1p' f.txt"), "beta 2\n# note\n[exit code 0]")
        self.assertEqual(self.run("sed '1a after\n1i before' f.txt | head -3"),
                         "before\nalpha 1\nafter\n[exit code 0]")
        self.assertEqual(self.run("sed '/beta/c\\\nreplaced' f.txt | head -2"), "alpha 1\nreplaced\n[exit code 0]")
        self.assertEqual(self.run("sed -n '/alpha/{s/1/one/;p}' f.txt"), "alpha one\n[exit code 0]")
        self.assertEqual(self.sed("'y/abc/xyz/'", "aabbcc\n"), "xxyyzz\n[exit code 0]")
        self.assertEqual(self.sed("':a;N;$!ba;s/\\n/,/g'", "1\n2\n3\n"), "1,2,3\n[exit code 0]")
        self.assertEqual(self.sed("'$!N;P;D'", "a\nb\nc\n"), "a\nb\nc\n[exit code 0]")
        self.assertEqual(self.sed("-n 'h;n;G;p'", "1\n2\n3\n4\n"), "2\n1\n4\n3\n[exit code 0]")
        self.assertEqual(self.run("printf 'no newline' | sed 's/no/a/'"), "a newline\n[exit code 0]")

    def test_in_place(self):
        self.run("sed -i 's/beta/BETA/' f.txt")
        self.assertIn("BETA 2", self.get("f.txt"))
        self.run("sed -i.bak '1d' f.txt")
        self.assertTrue(self.get("f.txt").startswith("BETA"))
        self.assertTrue(self.get("f.txt.bak").startswith("alpha"))
        self.run("sed -i '' 's/BETA/beta/' f.txt")
        self.assertTrue(self.get("f.txt").startswith("beta"))
        self.put("x.txt", "a\nb")
        self.run("sed -i 's/b/c/' x.txt")
        self.assertEqual(self.get("x.txt"), "a\nc")

    def test_errors(self):
        self.assertIn("unterminated", self.run("sed 's/a/b' f.txt"))
        self.assertIn("unknown command", self.run("sed 'k' f.txt"))
        self.assertIn("outside the workspace", self.run("sed -i s/a/b/ /etc/passwd"))


class Awk(Workspace):
    def setUp(self):
        super().setUp()
        self.run = load("run_command")
        self.put("d.csv", "name,dept,pay\nann,eng,100\nbob,ops,80\ncat,eng,120\n")

    def awk(self, prog, data=None, opts=""):
        if data is not None:
            self.put("in.txt", data)
        return self.run(f"awk {opts} '{prog}' " + ("in.txt" if data is not None else "d.csv"))

    def test_fields_patterns_and_end(self):
        self.assertEqual(self.awk("{print $1}", opts="-F,"), "name\nann\nbob\ncat\n[exit code 0]")
        self.assertEqual(self.awk("NR > 1 && $3 >= 100 {print $1}", opts="-F,"), "ann\ncat\n[exit code 0]")
        self.assertEqual(self.awk("NR>1 {s += $3} END {print s, s/(NR-1)}", opts="-F,"), "300 100\n[exit code 0]")
        self.assertEqual(self.awk("/eng/", opts="-F,"), "ann,eng,100\ncat,eng,120\n[exit code 0]")
        self.assertEqual(self.awk("$2 ~ /^o/ {print NR\": \"$0}", opts="-F,"), "3: bob,ops,80\n[exit code 0]")
        self.assertEqual(self.awk("{print NF, $NF}", "a b c\n  d  \n"), "3 c\n1 d\n[exit code 0]")
        self.assertEqual(self.awk("{$2 = \"X\"; print}", "a b c\n"), "a X c\n[exit code 0]")
        self.assertEqual(self.awk("BEGIN {OFS=\"-\"} {$1=$1; print}", "a b c\n"), "a-b-c\n[exit code 0]")
        self.assertEqual(self.awk("{print $(NF-1)}", "a b c\n"), "b\n[exit code 0]")
        self.assertEqual(self.awk("NR==2, NR==3 {print $1}", opts="-F,"), "ann\nbob\n[exit code 0]")
        self.assertEqual(self.awk("{print $2}", "a\tb c\n", opts="-F'\\t'"), "b c\n[exit code 0]")

    def test_arrays_loops_and_functions(self):
        r = self.awk("NR>1 {n[$2]++; t[$2]+=$3} END {for (d in n) printf \"%s %d %.1f\\n\", d, n[d], t[d]/n[d]}",
                     opts="-F,")
        self.assertEqual(r, "eng 2 110.0\nops 1 80.0\n[exit code 0]")
        self.assertEqual(self.awk("BEGIN {for (i = 1; i <= 3; i++) s = s i; print s}"), "123\n[exit code 0]")
        self.assertEqual(self.awk("BEGIN {n = split(\"a:b:c\", p, \":\"); print n, p[3]; delete p[3]; print length(p)}"),
                         "3 c\n2\n[exit code 0]")
        self.assertEqual(self.awk("BEGIN {x = 3; while (x > 0) {printf \"%d \", x--}; print \"\"}"), "3 2 1 \n[exit code 0]")
        self.assertEqual(self.awk("function sq(v) { return v * v } BEGIN { print sq(7) }"), "49\n[exit code 0]")
        self.assertEqual(self.awk("function fill(a) { a[\"k\"] = 1 } BEGIN { fill(m); print (\"k\" in m) }"),
                         "1\n[exit code 0]")
        self.assertEqual(self.awk("{ if ($1 > 5) print \"big\"; else print \"small\" }", "3\n9\n"),
                         "small\nbig\n[exit code 0]")
        self.assertEqual(self.awk("!seen[$0]++", "a\nb\na\n"), "a\nb\n[exit code 0]")
        self.assertEqual(self.awk("{ a[NR] = $0 } END { for (i = NR; i > 0; i--) print a[i] }", "1\n2\n"),
                         "2\n1\n[exit code 0]")
        self.assertEqual(self.awk("NR == 1 { next } { print } NR == 3 { exit }", "a\nb\nc\nd\n"), "b\nc\n[exit code 0]")

    def test_strings_and_numbers(self):
        self.assertEqual(self.awk("BEGIN {s = \"Hello World\"; print length(s), substr(s, 7), index(s, \"o\"), toupper(s)}"),
                         "11 World 5 HELLO WORLD\n[exit code 0]")
        self.assertEqual(self.awk("{gsub(/o/, \"0\"); sub(/l/, \"L\"); print}", "hello world\n"), "heLl0 w0rld\n[exit code 0]")
        self.assertEqual(self.awk("{n = gsub(/[aeiou]/, \"<&>\"); print n, $0}", "tea\n"), "2 t<e><a>\n[exit code 0]")
        self.assertEqual(self.awk("match($0, /[0-9]+/) {print RSTART, RLENGTH, substr($0, RSTART, RLENGTH)}",
                                  "abc 123 x\n"), "5 3 123\n[exit code 0]")
        self.assertEqual(self.awk("BEGIN {printf \"%5.2f|%-4s|%03d|%x|%c|%e\\n\", 3.14159, \"ab\", 7, 255, 65, 1234.5}"),
                         " 3.14|ab  |007|ff|A|1.234500e+03\n[exit code 0]")
        self.assertEqual(self.awk("BEGIN {print 1/3, 2^10, 7%3, int(-3.7), 1e3, 0.1+0.2}"),
                         "0.333333 1024 1 -3 1000 0.3\n[exit code 0]")
        self.assertEqual(self.awk("{print ($1 < $2)}", "10 9\n"), "0\n[exit code 0]")
        self.assertEqual(self.awk("BEGIN {print (\"10\" < \"9\"), (x == 0), (x == \"\")}"), "1 1 1\n[exit code 0]")
        self.assertEqual(self.awk("BEGIN {print sprintf(\"%s-%s\", \"a\", 1) \"!\"}"), "a-1!\n[exit code 0]")
        self.assertEqual(self.awk("{print $1 > \"out.txt\"}", "x y\n"), "(no output)\n[exit code 0]")
        self.assertEqual(self.get("out.txt"), "x\n")
        self.assertEqual(self.run("awk -v n=2 '{print $1 * n}' <<< 21"), "42\n[exit code 0]")
        self.assertEqual(self.run("echo 'a b' | awk '{print $2}'"), "b\n[exit code 0]")

    def test_errors(self):
        self.assertEqual(self.awk("{print $1"), "awk: a { isn't closed near the end\n[exit code 2]")
        self.assertIn("syntax error", self.awk("{print $1 +}"))
        self.assertIn("getline", self.awk("{getline x}"))
        self.assertIn("system()", self.awk("BEGIN {system(\"ls\")}"))
        self.assertIn("division by zero", self.awk("BEGIN {print 1/0}"))
        self.assertIn("outside the workspace", self.awk("BEGIN {print 1 > \"/etc/x\"}"))


class MoreCommands(Workspace):
    def setUp(self):
        super().setUp()
        self.run = load("run_command")

    def test_printf_echo_seq(self):
        self.assertEqual(self.run("printf '%s=%d\\n' a 1 b 2"), "a=1\nb=2\n[exit code 0]")
        self.assertEqual(self.run("printf '%-5s|%5.1f|%%\\n' ab 2.25"), "ab   |  2.2|%\n[exit code 0]")
        self.assertEqual(self.run("echo -e 'a\\tb'"), "a\tb\n[exit code 0]")
        self.assertEqual(self.run("echo -n x; echo y"), "xy\n[exit code 0]")
        self.assertEqual(self.run("seq 3"), "1\n2\n3\n[exit code 0]")
        self.assertEqual(self.run("seq -s, 2 2 8"), "2,4,6,8\n[exit code 0]")
        self.assertEqual(self.run("seq -w 8 10"), "08\n09\n10\n[exit code 0]")

    def test_xargs_and_find_exec(self):
        self.put("a.txt", "x\n")
        self.put("b.txt", "y\ny\n")
        self.put("sub/c.log", "z\n")
        self.assertEqual(self.run("ls *.txt | xargs wc -l"), "1 a.txt\n2 b.txt\n3 total\n[exit code 0]")
        self.assertEqual(self.run("ls *.txt | xargs -I {} echo file {}"), "file a.txt\nfile b.txt\n[exit code 0]")
        self.assertEqual(self.run("echo a b c | xargs -n 1 echo"), "a\nb\nc\n[exit code 0]")
        self.assertEqual(self.run("find . -name '*.txt' -exec grep -l y {} +"), "./b.txt\n[exit code 0]")
        self.assertEqual(self.run("find . -name '*.txt' -exec echo got {} \\;"), "got ./a.txt\ngot ./b.txt\n[exit code 0]")
        self.assertEqual(self.run("find . -type f -not -name '*.txt'"), "./sub/c.log\n[exit code 0]")
        self.assertEqual(self.run("find . -name '*.log' -o -name 'a*'"), "./a.txt\n./sub/c.log\n[exit code 0]")
        self.assertEqual(self.run("find sub -maxdepth 0"), "sub\n[exit code 0]")
        self.run("find . -name '*.log' -delete")
        self.assertFalse(os.path.exists(os.path.join(self.ws, "sub/c.log")))
        self.assertIn("command not found", self.run("echo x | xargs nosuch"))

    def test_text_tools(self):
        self.put("t.txt", "b 2\na 10\nc 1\na 10\n")
        self.assertEqual(self.run("sort -k2 -n t.txt"), "c 1\nb 2\na 10\na 10\n[exit code 0]")
        self.assertEqual(self.run("sort -t' ' -k2,2nr -u t.txt"), "a 10\nb 2\nc 1\n[exit code 0]")
        self.assertEqual(self.run("sort t.txt | uniq -c"), "      2 a 10\n      1 b 2\n      1 c 1\n[exit code 0]")
        self.assertEqual(self.run("sort t.txt | uniq -d"), "a 10\n[exit code 0]")
        self.assertEqual(self.run("tr '[:lower:]' '[:upper:]' < t.txt | head -1"), "B 2\n[exit code 0]")
        self.assertEqual(self.run("tr '\\n' ' ' < t.txt"), "b 2 a 10 c 1 a 10 \n[exit code 0]")
        self.assertEqual(self.run("echo 'aa  bb' | tr -s ' '"), "aa bb\n[exit code 0]")
        self.assertEqual(self.run("echo 'a1b2' | tr -cd '[:digit:]'"), "12\n[exit code 0]")
        self.assertEqual(self.run("tac t.txt | head -1"), "a 10\n[exit code 0]")
        self.assertEqual(self.run("echo abc | rev"), "cba\n[exit code 0]")
        self.assertEqual(self.run("nl t.txt | head -1"), "     1\tb 2\n[exit code 0]")
        self.assertEqual(self.run("seq 4 | paste -sd+"), "1+2+3+4\n[exit code 0]")
        self.assertEqual(self.run("printf 'a bb\\nccc d\\n' | column -t"), "a    bb\nccc  d\n[exit code 0]")
        self.assertEqual(self.run("echo hi | tee x.txt | wc -c"), "3\n[exit code 0]")
        self.assertEqual(self.get("x.txt"), "hi\n")

    def test_grep_context_and_more(self):
        self.put("g.txt", "one\ntwo\nthree\nfour\nfive\n")
        self.put("src/a.py", "TODO: x\n")
        self.put("src/b.md", "TODO: y\n")
        self.assertEqual(self.run("grep -n -A1 two g.txt"), "2:two\n3-three\n[exit code 0]")
        self.assertEqual(self.run("grep -B1 -m1 'f' g.txt"), "three\nfour\n[exit code 0]")
        self.assertEqual(self.run("grep -E 'one|five' g.txt"), "one\nfive\n[exit code 0]")
        self.assertEqual(self.run("grep 'o\\|v' g.txt | wc -l"), "4\n[exit code 0]")
        self.assertEqual(self.run("grep -rn --include='*.py' TODO src"), "src/a.py:1:TODO: x\n[exit code 0]")
        self.assertEqual(self.run("grep -q two g.txt && echo yes"), "yes\n[exit code 0]")
        self.assertEqual(self.run("grep -c six g.txt"), "0\n[exit code 1]")
        self.assertEqual(self.run("grep -x 'fo.r' g.txt"), "four\n[exit code 0]")

    def test_paths_tests_and_sums(self):
        self.put("d/x.txt", "hello\n")
        self.assertEqual(self.run("basename d/x.txt .txt; dirname d/x.txt"), "x\nd\n[exit code 0]")
        self.assertEqual(self.run("realpath d"), os.path.join(self.ws, "d") + "\n[exit code 0]")
        self.assertEqual(self.run("[ -f d/x.txt ] && echo file; test -d d && echo dir; [ -e nope ] || echo none"),
                         "file\ndir\nnone\n[exit code 0]")
        self.assertEqual(self.run("[ 3 -gt 2 ] && [ abc = abc ] && echo ok"), "ok\n[exit code 0]")
        self.assertEqual(self.run("[ 1 -eq 2 ]"), "(no output)\n[exit code 1]")
        self.assertEqual(self.run("sha256sum d/x.txt"),
                         "5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03  d/x.txt\n[exit code 0]")
        self.assertEqual(self.run("md5sum < d/x.txt"), "b1946ac92492d2347c6235b4d2611184  -\n[exit code 0]")
        self.assertEqual(self.run("echo hello | base64"), "aGVsbG8K\n[exit code 0]")
        self.assertEqual(self.run("echo aGVsbG8K | base64 -d"), "hello\n[exit code 0]")
        self.assertEqual(self.run("tree"), ".\n└── d/\n    └── x.txt\n\n1 directory, 1 file\n[exit code 0]")
        self.assertEqual(self.run("du -s d"), "1\td\n[exit code 0]")
        self.assertEqual(self.run("which sed awk nope"), "sed: built into this shell\nawk: built into this shell\nwhich: no nope here\n[exit code 1]")
        self.assertEqual(self.run("command -v grep"), "grep\n[exit code 0]")
        self.assertIn("outside the workspace", self.run("realpath /etc"))
        self.assertIn("outside the workspace", self.run("echo x | tee /tmp/x"))


class TerminalPromptExamples(Workspace):
    """Every command the Terminal assistant's prompt shows the model
    (crates/flow/templates/terminal.py) works in run_command."""

    EXAMPLES = [
        "ls -la", "tree -L 2", "find . -name '*.py'", "du -sh *",
        "cat -n f.txt", "head -n 40 f.txt", "sed -n '120,160p' f.txt", "grep -n 'def ' app.py",
        "grep -rn 'TODO' --include='*.py' .", "grep -rl 'word' .", "grep -c 'ERROR' app.log",
        "cat > plan.md <<'EOF'\n# Plan\n- first step\nEOF",
        "sed -i 's/old/new/g' f.txt", "sed -i '12s/.*/new line 12/' f.txt", "sed -i '/^#/d' f.txt",
        "python - <<'EOF'\nprint('edited')\nEOF",
        "awk -F, 'NR>1 {s[$2] += $3} END {for (k in s) print k, s[k]}' data.csv | sort",
        "cut -d, -f1,3 data.csv", "sort -t, -k3 -n data.csv", "sort data.csv | uniq -c | sort -rn",
        "find . -name '*.log' | xargs wc -l", "find src -name '*.py' -exec grep -l TODO {} +",
        "python script.py", "python -c \"print(2**10)\"", "help", "help sed", "sed --help",
        "echo {a,b} {1..3} | wc -w", "cat f.txt 2>/dev/null | head -1 > out.txt && cat out.txt",
    ]

    def test_examples_run(self):
        run = load("run_command")
        self.put("f.txt", "".join(f"line {i} old\n" for i in range(1, 200)) + "# comment\n")
        self.put("app.py", "def main():\n    pass  # TODO word\n")
        self.put("app.log", "ERROR one\nINFO two\n")
        self.put("data.csv", "name,dept,pay\nann,eng,100\nbob,ops,80\n")
        self.put("src/x.py", "# TODO\n")
        self.put("script.py", "print('ran')\n")
        for cmd in self.EXAMPLES:
            r = run(cmd)
            self.assertTrue(r.endswith("[exit code 0]"), f"{cmd}\n{r}")
        self.assertEqual(self.get("plan.md"), "# Plan\n- first step\n")
        self.assertIn("new line 12", self.get("f.txt"))
        self.assertNotIn("# comment", self.get("f.txt"))


if __name__ == "__main__":
    unittest.main()
