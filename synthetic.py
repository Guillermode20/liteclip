import requests
import json  # Import this

headers = {"Authorization": "Bearer syn_395b18c19c32f76e4545fd61ca6e3510"}
response = requests.get("https://api.synthetic.new/v2/quotas", headers=headers)

# Use json.dumps to format the dictionary
print(json.dumps(response.json(), indent=4))