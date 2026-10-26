def test_docs_cover_the_day():
    text = open("docs/CLI.md").read()
    assert "customers show" in text
