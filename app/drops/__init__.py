"""File drops - single-use, PIN-protected upload links.

A master creates a link in the dev portal, hands out the URL + a PIN, and the
person on the other end uploads a file without an account. The link dies on its
own: it expires, and it stops working after the number of uploads it was made
for. See ``app/drops/service.py`` for the rules.
"""
