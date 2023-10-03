#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: The Ferrocene Developers

import os
import requests
import sys
import subprocess


DEFAULT_REPOSITORY = "ferrocene/ferrocene"


def get_base_and_head(token, repository, pr_number):
    result = requests.get(
        f"https://api.github.com/repos/{repository}/pulls/{pr_number}",
        headers={"Authorization": f"token {token}"},
    )
    result.raise_for_status()
    json = result.json()
    return json["base"]["sha"], json["head"]["sha"]


def main():
    if len(sys.argv) != 2:
        print(f"usage: {sys.argv[0]} <pr-number>")
        exit(1)
    pr_number = sys.argv[1]

    current_dir = os.path.abspath(os.path.dirname(__file__))

    repository = os.environ.get("GITHUB_REPOSITORY", DEFAULT_REPOSITORY)
    try:
        token = os.environ["GITHUB_TOKEN"]
    except KeyError:
        print("error: a GitHub API token needs to be set in the GITHUB_TOKEN env var")
        exit(1)

    base, head = get_base_and_head(token, repository, pr_number)

    current_branch = subprocess.run(
        ["git", "rev-parse", "--abbrev-ref", "HEAD"],
        stdout=subprocess.PIPE,
        text=True,
        check=True,
    ).stdout.strip()
    # The command above returns "HEAD" when we're not in any named branch, so
    # treat that as not being in any branch.
    if current_branch == "HEAD":
        current_branch = ""

    result = subprocess.run(
        [
            "git",
            "rebase",
            # Customize the list of steps executed for this rebase. The user
            # won't be prompted with the editor to change the todo list though,
            # since we set the editor to run custom code (see the editor script
            # for a list of the changes we do).
            "--interactive",
            # The "exec" added by the editor needs to be executed successfully
            # for the rebase to go through. Ensure it's rescheduled on failure.
            "--reschedule-failed-exec",
            "--onto",
            current_branch,
            base,
            head,
        ],
        env={
            **os.environ,
            "FERROCENE_PR_NUMBER": pr_number,
            "FERROCENE_CURRENT_BRANCH": current_branch,
            "GIT_SEQUENCE_EDITOR": os.path.join(
                current_dir, "utils", "rebase-interactive-editor.py"
            ),
        },
    )
    exit(result.returncode)


if __name__ == "__main__":
    main()
