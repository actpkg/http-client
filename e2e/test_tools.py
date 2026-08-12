async def test_component_exposes_the_fetch_tool(client):
    # The old suite had two hurl files asserting overlapping claims about
    # the same /tools listing (tools.hurl: count >= 1; list_tools.hurl: the
    # same count >= 1, plus "fetch" is in the names) — list_tools.hurl's
    # assertions are a strict superset of tools.hurl's, so this one test
    # covers both files rather than duplicating the identical count check.
    tools = await client.list_tools()
    assert len(tools) >= 1
    assert "fetch" in [t.name for t in tools]
