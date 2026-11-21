# One-word entry points for any agent or person. Details: AGENTS.md, tools/check.sh.
.PHONY: help check test lint check-docs release-plan release-verify e2e-podman logs
SHELL := /usr/bin/env bash
export PYTHONDONTWRITEBYTECODE := 1

PYTHON ?= python3
JOBS ?= auto
SLICE ?=
HOST_TESTS := tests/version-test.sh tests/release-test.sh tests/install-bootstrap-test.sh tests/tmux-test.sh tests/fail2ban-sync-test.sh tests/agent-fmt-test.sh

help:
	@echo "make check           run all machine-gate checks in parallel (tools/check.sh)"
	@echo "make check SLICE=p   run fast scoped check for path(s) (comma-separated)"
	@echo "make test            run unprivileged host unit tests (tests/*)"
	@echo "make lint            run bash -n, shellcheck (if installed), and repo hygiene"
	@echo "make check-docs      verify agent-only and human-facing doc format (verify_agent_fmt.py)"
	@echo "make release-plan    preview next SemVer release & changelog (tools/release.sh plan)"
	@echo "make release-verify  verify VERSION, VERSION_ID, and CHANGELOG.md consistency"
	@echo "make logs            list check logs in $${DEBOPTY_LOG_DIR:-/tmp/debopty_logs}"

check:
	DEBOPTY_JOBS=$(JOBS) DEBOPTY_SLICE="$(SLICE)" bash tools/check.sh

test:
	@rc=0; ran=0; \
	for t in $(HOST_TESTS); do \
		if [ -f "$$t" ]; then \
			echo "== $$t"; \
			bash "$$t" || rc=1; \
			ran=$$((ran + 1)); \
		fi; \
	done; \
	if [ "$$ran" -eq 0 ]; then \
		echo "SKIP: no host unit tests present"; \
	fi; \
	exit $$rc

lint:
	@rc=0; \
	dirs=(); \
	for d in deb_optimizer.sh install.sh scripts templates tools tests; do \
		[ -e "$$d" ] && dirs+=("$$d"); \
	done; \
	if [ "$${#dirs[@]}" -gt 0 ]; then \
		while IFS= read -r -d '' f; do \
			bash -n "$$f" || rc=1; \
		done < <(find "$${dirs[@]}" -type f -name '*.sh' -print0); \
	fi; \
	if command -v shellcheck >/dev/null 2>&1; then \
		shopt -s nullglob; \
		sc_files=(deb_optimizer.sh install.sh scripts/*.sh scripts/apps/*.sh tools/*.sh); \
		if [ "$${#sc_files[@]}" -gt 0 ]; then \
			shellcheck "$${sc_files[@]}" || rc=1; \
		fi; \
	else \
		echo "SKIP: shellcheck not installed"; \
	fi; \
	$(PYTHON) -B tools/verify_repo_hygiene.py || rc=1; \
	exit $$rc

check-docs:
	$(PYTHON) -B tools/verify_agent_fmt.py

release-plan:
	bash tools/release.sh plan

release-verify:
	bash tools/release.sh verify

e2e-podman:
	bash tests/e2e-systemd-test.sh

logs:
	@ls -lh "$${DEBOPTY_LOG_DIR:-$${TMPDIR:-/tmp}/debopty_logs}" 2>/dev/null || echo "no logs yet"
