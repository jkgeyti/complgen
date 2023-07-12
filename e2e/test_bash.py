import os
import sys
import tempfile
import contextlib
import subprocess
from pathlib import Path
from typing import Generator

import pytest

from conftest import set_working_dir


@contextlib.contextmanager
def completion_script_path(complgen_binary_path: Path, grammar: str) -> Generator[Path, None, None]:
    bash_script = subprocess.run([complgen_binary_path, 'compile', '--bash-script', '-', '-'], input=grammar.encode(), stdout=subprocess.PIPE, stderr=sys.stderr, check=True).stdout
    with tempfile.NamedTemporaryFile() as f:
        f.write(bash_script)
        f.flush()
        yield Path(f.name)


def get_sorted_completions(completions_file_path: Path, input: str) -> list[str]:
    bash_process = subprocess.run(['bash', '--noprofile', '--rcfile', completions_file_path, '-i'], input=input.encode(), stdout=subprocess.PIPE, stderr=sys.stderr, check=True)
    lines = bash_process.stdout.decode().splitlines()
    lines.sort()
    return lines


SPECIAL_CHARACTERS = '?[^a]*{foo,*bar}'


def test_completes_paths(complgen_binary_path: Path):
    with completion_script_path(complgen_binary_path, '''cmd <PATH> [--help];''') as completions_file_path:
        with tempfile.TemporaryDirectory() as dir:
            with set_working_dir(Path(dir)):
                Path('foo').write_text('dummy')
                Path(SPECIAL_CHARACTERS).write_text('dummy')
                os.mkdir('baz')
                completions = get_sorted_completions(completions_file_path, '''COMP_WORDS=(cmd); COMP_CWORD=1; _cmd; printf '%s\n' "${COMPREPLY[@]}"''')
                assert completions == sorted(['foo', SPECIAL_CHARACTERS, 'baz'])


def test_completes_directories(complgen_binary_path: Path):
    with completion_script_path(complgen_binary_path, '''cmd <DIRECTORY> [--help];''') as completions_file_path:
        with tempfile.TemporaryDirectory() as dir:
            with set_working_dir(Path(dir)):
                os.mkdir('foo')
                os.mkdir(SPECIAL_CHARACTERS)
                Path('baz').write_text('dummy')
                completions = get_sorted_completions(completions_file_path, '''COMP_WORDS=(cmd); COMP_CWORD=1; _cmd; printf '%s\n' "${COMPREPLY[@]}"''')
                assert completions == sorted(['foo', SPECIAL_CHARACTERS, 'bar'])


def test_bash_uses_correct_transition_with_duplicated_literals(complgen_binary_path: Path):
    GRAMMAR = '''
cmd <COMMAND> [--help];

<COMMAND> ::= rm           "Remove a project" <RM-OPTION>
            | remote       "Manage a project's remotes" [<REMOTE-SUBCOMMAND>]
            ;

<REMOTE-SUBCOMMAND> ::= rm <name>;
'''

    with completion_script_path(complgen_binary_path, GRAMMAR) as completions_file_path:
        assert get_sorted_completions(completions_file_path, r'''COMP_WORDS=(cmd remote); COMP_CWORD=2; _cmd; if [[ ${#COMPREPLY[@]} -gt 0 ]]; then printf '%s\n' "${COMPREPLY[@]}"; fi''') == sorted(['--help', 'rm'])
        assert get_sorted_completions(completions_file_path, r'''COMP_WORDS=(cmd rm); COMP_CWORD=2; _cmd; if [[ ${#COMPREPLY[@]} -gt 0 ]]; then printf '%s\n' "${COMPREPLY[@]}"; fi''') == sorted([])


def test_bash_external_command_produces_description(complgen_binary_path: Path):
    GRAMMAR = r'''
cmd { echo -e "completion\tdescription" };
'''
    with completion_script_path(complgen_binary_path, GRAMMAR) as path:
        input = r'''COMP_WORDS=(cmd); COMP_CWORD=1; _cmd; printf '%s\n' "${COMPREPLY[@]}"'''
        assert get_sorted_completions(path, input) == sorted(['completion'])


def test_jit_completes(complgen_binary_path: Path):
    GRAMMAR = '''cmd (--help | --version); '''
    process = subprocess.run([complgen_binary_path, 'complete', '-', 'bash', '--', '0'], input=GRAMMAR.encode(), stdout=subprocess.PIPE, stderr=sys.stderr, check=True)
    assert sorted(process.stdout.decode().splitlines()) == sorted(['--help', '--version'])


def test_jit_completes_prefix(complgen_binary_path: Path):
    GRAMMAR = '''cmd (--help | --version); '''
    process = subprocess.run([complgen_binary_path, 'complete', '-', 'bash', '--', '0', '--h'], input=GRAMMAR.encode(), stdout=subprocess.PIPE, stderr=sys.stderr, check=True)
    assert sorted(process.stdout.decode().splitlines()) == sorted(['--help'])


def get_sorted_jit_bash_completions(complgen_binary_path: Path, grammar: str, completed_word_index: int, words_before_cursor: list[str]) -> list[str]:
    process = subprocess.run([complgen_binary_path, 'complete', '-', 'bash', '--', str(completed_word_index)] + words_before_cursor, input=grammar.encode(), stdout=subprocess.PIPE, stderr=sys.stderr, check=True)
    lines = process.stdout.decode().splitlines()
    return sorted(lines)


def test_jit_completes_paths_bash(complgen_binary_path: Path):
    with tempfile.TemporaryDirectory() as dir:
        with set_working_dir(Path(dir)):
            Path('foo').write_text('dummy')
            Path(SPECIAL_CHARACTERS).write_text('dummy')
            os.mkdir('baz')
            assert get_sorted_jit_bash_completions(complgen_binary_path, '''cmd <PATH> [--help];''', 0, []) == sorted([SPECIAL_CHARACTERS, 'foo', 'baz'])


def test_jit_completes_directories_bash(complgen_binary_path: Path):
    with tempfile.TemporaryDirectory() as dir:
        with set_working_dir(Path(dir)):
            os.mkdir('foo')
            os.mkdir(SPECIAL_CHARACTERS)
            Path('baz').write_text('dummy')
            assert get_sorted_jit_bash_completions(complgen_binary_path, '''cmd <DIRECTORY> [--help];''', 0, []) == sorted([SPECIAL_CHARACTERS, 'foo'])


def test_specializes_for_bash(complgen_binary_path: Path):
    GRAMMAR = '''cmd <FOO>; <FOO> ::= { echo foo }; <FOO@bash> ::= { echo bash };'''
    with completion_script_path(complgen_binary_path, GRAMMAR) as path:
        input = r'''COMP_WORDS=(cmd); COMP_CWORD=1; _cmd; printf '%s\n' "${COMPREPLY[@]}"'''
        assert get_sorted_completions(path, input) == sorted(['bash'])


def test_jit_specializes_for_bash(complgen_binary_path: Path):
    GRAMMAR = '''cmd <FOO>; <FOO> ::= { echo foo }; <FOO@bash> ::= { echo bash };'''
    assert get_sorted_jit_bash_completions(complgen_binary_path, GRAMMAR, 0, []) == sorted(['bash'])
