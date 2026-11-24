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
        self.assertIn("isn't supported", self.run("find . -exec rm"))
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
        self.assertIn("npm: command not found", r)
        self.assertIn("python", r)
        self.assertTrue(r.endswith("[exit code 127]"))
        self.assertIn("can't be installed", self.run("pip install requests"))
        self.assertEqual(self.run('bash -c "echo inner"'), "inner\n[exit code 0]")


if __name__ == "__main__":
    unittest.main()


@unittest.skipIf(os.name == "nt", "needs sh")
class RunShell(Workspace):
    def setUp(self):
        super().setUp()
        self.run = load("run_shell")

    def test_runs_in_the_workspace_with_exit_codes(self):
        self.put("a.txt", "hello\n")
        self.assertEqual(self.run("cat a.txt; pwd"), f"hello\n{self.ws}\n[exit code 0]")
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
