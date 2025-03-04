#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: The Ferrocene Developers

# This script is responsible for calculating the list of release jobs we should
# start, as part of the .github/workflows/release.yml GitHub Actions workflow.

from dataclasses import dataclass
import base64
import boto3
import collections
import json
import os
import re
import requests
import sys


# Regex matching all branches that should be released by our tooling.
RELEASE_BRANCHES_RE = re.compile(r"^(main|release\/1\.[0-9]+)$")

S3_BUCKET = "ferrocene-ci-artifacts"
S3_PREFIX = "ferrocene/dist"


def commits_in_release_branches(ctx):
    commits = []

    # This is an unconditional note that's always emitted. We do that to give
    # insights to someone looking at the logs to figure out why a branch isn't
    # being picked up.
    print("note: only protected branches are considered here", file=sys.stderr)

    url = (
        f"https://api.github.com/repos/{ctx.repo}/branches?protected=true&per_page=100"
    )
    while url is not None:
        response = ctx.http.get(url)
        response.raise_for_status()

        # Handle pagination
        if "next" in response.links and response.links["next"]["url"] is not None:
            url = response.links["next"]["url"]
        else:
            url = None

        for branch in response.json():
            name = branch["name"]
            if RELEASE_BRANCHES_RE.search(name) is None:
                print(
                    f"note: branch `{name}` doesn't seem to be a release branch",
                    file=sys.stderr,
                )
            else:
                commits.append(branch["commit"]["sha"])

    return commits


def resolve_ref(ctx, ref):
    response = ctx.http.get(f"https://api.github.com/repos/{ctx.repo}/commits/{ref}")
    response.raise_for_status()
    return response.json()["sha"]


def commits_to_releases(ctx, all_commits):
    for commit in all_commits:
        metadata = build_metadata(ctx, commit)
        yield PendingRelease(commit, metadata)


def filter_automated_channels(releases):
    rolling_releases = []
    for release in releases:
        if release.metadata.channel == "nightly":
            yield release
        elif release.metadata.channel == "pre-rolling":
            yield release
        elif release.metadata.channel.startswith("beta-"):
            yield release
        elif release.metadata.channel == "rolling":
            version = [int(num) for num in release.metadata.rust_version.split(".")]
            rolling_releases.append((version, release))
        else:
            print(
                "note: channel {release.metadata.channel} cannot be released automatically",
                file=sys.stderr,
            )

    rolling_releases.sort(key=lambda vr: vr[0])
    # When starting from a squashed repo with no release branches yielding the
    # latest rolling release would crash the script.
    if rolling_releases:
        yield rolling_releases.pop()[1]
    for discarded in rolling_releases:
        print(
            f"note: version {discarded[1].metadata.rust_version} is not the latest "
            "in the rolling channel",
            file=sys.stderr,
        )


# In some cases, it's possible that multiple pending releases target the same
# channel. This could happen for example if a new `release/1.NN` branch is
# created by an automation, and the PR bumping the channel away from nightly
# hasn't been merged yet. In that case, two releases with the same (nightly)
# channel will be attempted.
#
# Multiple pending releases with the same channel results in unpredictable
# behavior though. The release process would be instructed to release both of
# them, but the startup check preventing duplicate releases with the same ID
# would prevent ONE of them from being releases.
#
# This leads to unpredictability, since which commit ends up being released is
# not deterministic and only depends on which GitHub Actions job ran first.
# This could lead for example to a release channel going back in time.
#
# To prevent this from happening, the function discards all releases that have
# a duplicate channel. In the case above, no nightly release would be returned
# by the function, as two of them were pending.
def discard_duplicate_channels(releases):
    # Buffer the iterator as we need to iterate through it multiple times.
    releases = list(releases)

    channels_count = collections.defaultdict(lambda: 0)
    for release in releases:
        channels_count[release.metadata.channel] += 1

    for release in releases:
        if channels_count[release.metadata.channel] > 1:
            print(
                f"note: discarding {release.commit} on channel {release.metadata.channel} "
                "as multiple releases with that channel exist",
                file=sys.stderr,
            )
        else:
            yield release


def prepare_github_actions_output(ctx, pending_releases):
    if ctx.event_name == "schedule":
        environment = "release-prod-automated"
        name_suffix = "automated"
        command_suffix = ""
    elif ctx.event_name == "workflow_dispatch":
        inputs = ctx.event_data["inputs"]

        environment = f"release-{inputs['env']}-manual"
        name_suffix = "manual"
        command_suffix = ""

        if inputs["override-existing"] == "true":
            command_suffix += " --allow-overriding-existing-releases"
            name_suffix += ", allow overriding"
        if inputs["allow-duplicate"] == "true":
            command_suffix += " --allow-duplicate-releases"
            name_suffix += ", allow duplicates"

    jobs = []
    for release in discard_duplicate_channels(pending_releases):
        jobs.append(
            {
                "name": f"{release.metadata.channel} ({name_suffix})",
                "environment": environment,
                "command": f"ferrocene/ci/scripts/publish-release.sh {release.commit}{command_suffix}",
            }
        )
    return jobs


def build_metadata(ctx, commit):
    response = ctx.s3.get_object(
        Bucket=S3_BUCKET, Key=f"{S3_PREFIX}/{commit}/ferrocene-ci-metadata.json"
    )
    metadata = json.loads(response["Body"].read())

    def rustc_to_channel_rolling(rust_channel):
        try:
            return {
                "nightly": "nightly",
                "beta": "pre-rolling",
                "stable": "rolling",
            }[rust_channel]
        except:
            raise RuntimeError(f"unknown rust channel `{rust}`")

    if  metadata["metadata_version"] == 2:
        rust_channel=metadata["rust_channel"]
        ferrocene_channel=metadata["ferrocene_channel"]
        ferrocene_version=metadata["ferrocene_version"]
        rust_version=metadata["rust_version"]

        if ferrocene_version == "rolling":
            ferrocene_major = ferrocene_version
        else:
            ferrocene_major = ".".join(ferrocene_version.split(".")[:2])

        if ferrocene_channel == "rolling":
            channel = rustc_to_channel_rolling(metadata["rust_channel"])
        elif ferrocene_channel == "beta":
            channel =  f"beta-{ferrocene_major}"
        elif ferrocene_channel == "stable":
            channel = f"stable-{ferrocene_major}"
        else:
            raise RuntimeError(f"unknown ferrocene channel `{ferrocene_channel}`")

        return BuildMetadata(
            rust_version=rust_version,
            rust_channel=rust_channel,
            ferrocene_channel=ferrocene_channel,
            ferrocene_version=ferrocene_version,
            channel=channel
        )
    elif metadata["metadata_version"] == 3:
        return BuildMetadata(
            rust_version=metadata["rust_version"],
            rust_channel=metadata["rust_channel"],
            ferrocene_channel=metadata["ferrocene_channel"],
            ferrocene_version=metadata["ferrocene_version"],
            channel=metadata["channel"]
        )
    else:
        raise RuntimeError(
            "unexpected ferrocene-ci-metadata.json version "
            f"`{metadata['metadata_version']}` (for commit `{commit}`)"
        )


def run():
    s3 = boto3.client("s3")

    http = requests.Session()
    http.headers["Authorization"] = f"token {os.environ['GITHUB_TOKEN']}"

    repo = os.environ["GITHUB_REPOSITORY"]
    event_name = os.environ["GITHUB_EVENT_NAME"]
    event_path = os.environ["GITHUB_EVENT_PATH"]

    with open(event_path) as f:
        event_data = json.load(f)

    ctx = Context(
        s3=s3, http=http, repo=repo, event_name=event_name, event_data=event_data
    )

    if ctx.event_name == "schedule":
        commits = commits_in_release_branches(ctx)
        releases = filter_automated_channels(commits_to_releases(ctx, commits))
    elif ctx.event_name == "workflow_dispatch":
        if ctx.event_data["inputs"]["verbatim-ref"] == "true":
            commit = ctx.event_data["inputs"]["ref"]
        else:
            commit = resolve_ref(ctx, ctx.event_data["inputs"]["ref"])
        releases = commits_to_releases(ctx, [commit])
    else:
        raise RuntimeError(f"unsupported event name: {event_name}")

    output = prepare_github_actions_output(ctx, releases)
    print(f"jobs={json.dumps(output)}")


@dataclass
class BuildMetadata:
    rust_version: str
    rust_channel: str
    ferrocene_channel: str
    ferrocene_version: str
    channel: str


@dataclass
class PendingRelease:
    commit: str
    metadata: BuildMetadata


@dataclass
class Context:
    s3: object
    http: object
    repo: str
    event_name: str
    event_data: object


if __name__ == "__main__":
    run()
