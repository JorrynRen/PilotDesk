import os
base = r"E:\LingXi Workspace\20260530-10-40-29-689\pilotdesk\src"
for r, d, f in os.walk(base):
    for fn in f:
        if any(k in fn.lower() for k in ["titlebar", "title", "header", "app"]):
            print(os.path.join(r, fn))
