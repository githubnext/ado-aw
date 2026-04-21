"""List Azure DevOps projects using the REST API."""

import base64
import json
import os
import sys
import urllib.request

ORG = "msazuresphere"
API_VERSION = "7.1"

pat = os.environ.get("AZURE_DEVOPS_EXT_PAT") or input("Enter your ADO PAT: ")
creds = base64.b64encode(f":{pat}".encode()).decode()

url = f"https://dev.azure.com/{ORG}/_apis/projects?api-version={API_VERSION}"
req = urllib.request.Request(url, headers={"Authorization": f"Basic {creds}"})

try:
    with urllib.request.urlopen(req) as resp:
        data = json.loads(resp.read())
except urllib.error.HTTPError as e:
    print(f"HTTP {e.code}: {e.reason}", file=sys.stderr)
    sys.exit(1)

print(f"Found {data['count']} project(s):\n")
for p in data["value"]:
    print(f"  - {p['name']}  (id: {p['id']})")
