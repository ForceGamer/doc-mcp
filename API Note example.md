%%
endpoint=https://foo.bar/endpoint
headers={"Authorization": "Bearer ..."}
schema={"foo":{"bar":string, "baz":number}}
template:
# My name is {foo.bar}
And I am {foo.baz} years old!
%%

# My name is Name
And I am 30 years old!