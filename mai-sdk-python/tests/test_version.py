"""Verify the MAI SDK package is importable and has a version."""

from mai import __version__


def test_version_exists() -> None:
    """SDK version string is set."""
    assert isinstance(__version__, str)
    assert len(__version__) > 0
