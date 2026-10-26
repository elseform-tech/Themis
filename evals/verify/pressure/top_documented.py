def test_top_is_documented():
    text = open("docs/CLI.md").read()
    assert "reports top" in text and "--limit" in text
