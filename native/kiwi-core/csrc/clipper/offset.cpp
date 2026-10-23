// pyclipper's PyclipperOffset() + AddPath(JT_ROUND, ET_CLOSEDPOLYGON) + Execute(delta).
#include "clipper.hpp"
#include <cstdint>
#include <cstddef>

using namespace ClipperLib;

extern "C" long long kiwi_clipper_offset_round(const long long* xy, size_t n, double delta, long long* out,
                                               size_t out_cap, size_t* lens, size_t lens_cap, size_t* npaths)
{
    try {
        Path path;
        for (size_t i = 0; i < n; i++)
            path.push_back(IntPoint(xy[2 * i], xy[2 * i + 1]));
        ClipperOffset offset(2.0, 0.25);
        offset.AddPath(path, jtRound, etClosedPolygon);
        Paths solution;
        offset.Execute(solution, delta);
        size_t total = 0;
        for (size_t p = 0; p < solution.size(); p++)
            total += solution[p].size();
        *npaths = solution.size();
        if (total > out_cap || solution.size() > lens_cap)
            return (long long)total;
        size_t k = 0;
        for (size_t p = 0; p < solution.size(); p++) {
            lens[p] = solution[p].size();
            for (size_t i = 0; i < solution[p].size(); i++) {
                out[k++] = solution[p][i].X;
                out[k++] = solution[p][i].Y;
            }
        }
        return (long long)total;
    } catch (...) {
        return -1;
    }
}
