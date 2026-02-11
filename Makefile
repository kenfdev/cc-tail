.PHONY: setup

## Configure git hooks and development environment
setup:
	git config core.hooksPath .githooks
	@echo "Done. Git hooks now use .githooks/"
