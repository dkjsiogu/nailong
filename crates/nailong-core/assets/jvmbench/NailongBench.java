/*
 * nailong 通用调参测试。
 *
 * 不开窗口、不需要游戏文件，按 Minecraft 客户端的内存和线程特征造一份负载：
 *
 *   常驻数据   互相引用的小对象和中等数组，像注册表、方块状态、模型和纹理，一直活着。
 *   区块缓存   每个区块约 80 KB，生成线程不断造新区块替换旧的。区块活得够久，
 *              会被晋升到老年代，再在那里死去，老年代回收要靠它们来检验。
 *   生成线程   按固定速率生成区块，同时产生大量临时数组（噪声、结构）。
 *   服务端     每 50 ms 一个 tick：更新实体、读世界数据、产生临时对象。
 *   渲染       不封顶帧率，每帧读区块、拼顶点缓冲。回收停顿直接拉长这一帧，
 *              并发回收抢走的 CPU 和读屏障会拉低平均帧率。
 *
 * 速率都是固定的，所以不同的电脑跑出来的数能互相比较；同一台电脑上换回收器，
 * 帧率、tick 耗时和生成速度的差别就是回收器的影响。
 *
 * 编译（产物 NailongBench.class 提交进仓库，构建启动器不需要 JDK）：
 *   javac --release 8 -g:none -encoding UTF-8 -d . NailongBench.java
 *
 * 参数：预热秒 测量秒 常驻MB 缓存MB 每秒区块数 tick垃圾MB 生成线程数
 * 标准输出：
 *   nailong-bench:ready    数据建好
 *   nailong-bench:start    预热结束，开始测量
 *   nailong-bench:done     测量结束
 *   nailong-bench:result k=v ...
 */
import java.util.Arrays;
import java.util.concurrent.atomic.AtomicLong;
import java.util.concurrent.locks.LockSupport;

public final class NailongBench {
    static volatile boolean measuring;
    static volatile boolean stop;
    static volatile long sink;

    static Object[] nodes;
    static Object[] textures;
    static Object[] chunks;
    static Object[] entities;

    static long[] frames = new long[1 << 20];
    static int frameCount;
    static long[] ticks = new long[1 << 14];
    static int tickCount;
    static final AtomicLong made = new AtomicLong();

    public static void main(String[] args) throws Exception {
        int warmup = Integer.parseInt(args[0]);
        int seconds = Integer.parseInt(args[1]);
        int worldMb = Integer.parseInt(args[2]);
        int cacheMb = Integer.parseInt(args[3]);
        final int chunksPerSec = Integer.parseInt(args[4]);
        final int tickGarbageMb = Integer.parseInt(args[5]);
        int workers = Integer.parseInt(args[6]);

        buildWorld(worldMb);
        buildCache(cacheMb);
        entities = new Object[4000];
        for (int i = 0; i < entities.length; i++) {
            entities[i] = newEntity(i);
        }
        System.out.println("nailong-bench:ready");
        System.out.flush();

        Thread[] threads = new Thread[workers + 2];
        for (int w = 0; w < workers; w++) {
            final int id = w;
            final double rate = chunksPerSec / (double) workers;
            threads[w] = new Thread(() -> worker(id, rate), "worldgen-" + w);
        }
        threads[workers] = new Thread(() -> server(tickGarbageMb), "server");
        threads[workers + 1] = new Thread(NailongBench::render, "render");
        for (Thread t : threads) {
            t.setDaemon(true);
            t.start();
        }

        Thread.sleep(warmup * 1000L);
        frameCount = 0;
        tickCount = 0;
        made.set(0);
        long start = System.nanoTime();
        measuring = true;
        System.out.println("nailong-bench:start");
        System.out.flush();
        Thread.sleep(seconds * 1000L);
        measuring = false;
        long elapsed = System.nanoTime() - start;
        System.out.println("nailong-bench:done");
        System.out.flush();
        stop = true;

        // 读一次 volatile，确保看到渲染和服务端线程写下的计数。
        long seen = sink;
        double secs = elapsed / 1e9 + (seen & 0);
        long[] f = Arrays.copyOf(frames, Math.min(frameCount, frames.length));
        long[] t = Arrays.copyOf(ticks, Math.min(tickCount, ticks.length));
        Arrays.sort(f);
        Arrays.sort(t);
        long median = pick(f, 0.5);
        int hitches = 0;
        for (long x : f) {
            // 比平时多出半帧（60 FPS 下 8 ms）以上、而且慢了一半，玩家能看出这一下顿。
            if (x - median > 8_000_000L && x * 2 > median * 3) {
                hitches++;
            }
        }
        StringBuilder out = new StringBuilder("nailong-bench:result");
        out.append(" secs=").append(fmt(secs));
        out.append(" frames=").append(f.length);
        out.append(" frame_p50=").append(ms(median));
        out.append(" frame_p99=").append(ms(pick(f, 0.99)));
        out.append(" frame_max=").append(ms(f.length == 0 ? 0 : f[f.length - 1]));
        out.append(" hitches=").append(hitches);
        out.append(" ticks=").append(t.length);
        out.append(" tick_p50=").append(ms(pick(t, 0.5)));
        out.append(" tick_p95=").append(ms(pick(t, 0.95)));
        out.append(" tick_max=").append(ms(t.length == 0 ? 0 : t[t.length - 1]));
        out.append(" chunks=").append(made.get());
        out.append(" chunk_target=").append(fmt(chunksPerSec * secs));
        out.append(" sink=").append(sink & 0xff);
        System.out.println(out);
        System.out.flush();
        System.exit(0);
    }

    static long pick(long[] sorted, double q) {
        if (sorted.length == 0) {
            return 0;
        }
        int i = (int) Math.min(sorted.length - 1, Math.floor(sorted.length * q));
        return sorted[i];
    }

    static String ms(long nanos) {
        return fmt(nanos / 1e6);
    }

    static String fmt(double v) {
        return String.format(java.util.Locale.ROOT, "%.3f", v);
    }

    static long next(long x) {
        x ^= x << 13;
        x ^= x >>> 7;
        x ^= x << 17;
        return x;
    }

    static int below(long x, int bound) {
        return (int) ((x >>> 1) % bound);
    }

    /** 常驻数据：一半是互相引用的小对象，一半是 4 KB 的数组。 */
    static void buildWorld(int mb) {
        long bytes = mb * 1024L * 1024L;
        int nodeCount = (int) Math.max(1, bytes / 2 / 100);
        int textureCount = (int) Math.max(1, bytes / 2 / 4112);
        nodes = new Object[nodeCount];
        long r = 0x9E3779B97F4A7C15L;
        for (int i = 0; i < nodeCount; i++) {
            r = next(r);
            Object[] n = new Object[4];
            n[0] = new long[] {i, r};
            n[1] = new int[] {i, (int) r, (int) (r >>> 32), i ^ 0x5bd1e995};
            if (i > 0) {
                n[2] = nodes[below(r, i)];
                n[3] = nodes[below(r >>> 17, i)];
            }
            nodes[i] = n;
        }
        textures = new Object[textureCount];
        for (int i = 0; i < textureCount; i++) {
            int[] tex = new int[1024];
            for (int j = 0; j < tex.length; j += 64) {
                tex[j] = i * 31 + j;
            }
            textures[i] = tex;
        }
    }

    static void buildCache(int mb) {
        int count = (int) Math.max(16, mb * 1024L / 80);
        chunks = new Object[count];
        long r = 0x2545F4914F6CDD1DL;
        for (int i = 0; i < count; i++) {
            r = next(r);
            chunks[i] = newChunk(r, false);
        }
    }

    /** 一个区块：24 个分段，每段 128 个方块状态 long 和 2 KB 光照，再加 16 个方块实体。 */
    static Object[] newChunk(long seed, boolean generate) {
        Object[] sections = new Object[24];
        long r = seed | 1;
        for (int s = 0; s < 24; s++) {
            long[] states = new long[128];
            byte[] light = new byte[2048];
            if (generate) {
                // 噪声计算产生的临时数组，生成完就丢。
                double[] noise = new double[1024];
                for (int i = 0; i < noise.length; i++) {
                    r = next(r);
                    noise[i] = Math.sin((r & 0xffff) * 0.0001) * 0.5 + (i & 15) * 0.03;
                }
                for (int i = 0; i < states.length; i++) {
                    states[i] = Double.doubleToRawLongBits(noise[i * 8] + noise[(i * 8 + 3) & 1023]);
                }
                for (int i = 0; i < light.length; i += 16) {
                    light[i] = (byte) (noise[i & 1023] * 15);
                }
            } else {
                r = next(r);
                states[0] = r;
            }
            sections[s] = new Object[] {states, light};
        }
        Object[] blockEntities = new Object[16];
        for (int e = 0; e < blockEntities.length; e++) {
            blockEntities[e] = new Object[] {new int[8], new long[] {r, e}};
        }
        return new Object[] {sections, blockEntities, new long[] {seed}};
    }

    static Object[] newEntity(long seed) {
        return new Object[] {new double[6], new int[] {(int) seed, 20, 0, 0}};
    }

    static void park(long nanos) {
        if (nanos > 0) {
            LockSupport.parkNanos(Math.min(nanos, 5_000_000L));
        }
    }

    /** 生成线程：按固定速率造新区块，替换缓存里随机的一个。 */
    static void worker(int id, double perSecond) {
        long r = 0x632BE59BD9B4E019L + id * 0x9E3779B97F4A7C15L;
        long interval = (long) (1e9 / Math.max(0.1, perSecond));
        long next = System.nanoTime();
        while (!stop) {
            long now = System.nanoTime();
            if (now < next) {
                park(next - now);
                continue;
            }
            next += interval;
            // 落后超过一秒就不再追，免得测量开始时集中补一大批。
            if (now - next > 1_000_000_000L) {
                next = now;
            }
            r = next(r);
            Object[] chunk = newChunk(r, true);
            chunks[below(r, chunks.length)] = chunk;
            if (measuring) {
                made.incrementAndGet();
            }
        }
    }

    /** 服务端：每 50 ms 一个 tick，记录每个 tick 实际花的时间。 */
    static void server(int garbageMb) {
        long budget = garbageMb * 1024L * 1024L;
        long r = 0x5DEECE66DL;
        long next = System.nanoTime();
        while (!stop) {
            long now = System.nanoTime();
            if (now < next) {
                park(next - now);
                continue;
            }
            long started = now;
            long sum = 0;
            // 实体：全部移动一次，替换其中一成。
            for (int i = 0; i < entities.length; i++) {
                r = next(r);
                Object[] e = (Object[]) entities[i];
                double[] pos = (double[]) e[0];
                pos[0] += (r & 7) * 0.01;
                pos[1] = Math.max(0, pos[1] - 0.08);
                pos[2] += ((r >>> 3) & 7) * 0.01;
                if ((r & 15) == 0) {
                    entities[i] = newEntity(r);
                }
            }
            // 读世界：随机查注册表、方块状态。
            for (int i = 0; i < 4000; i++) {
                r = next(r);
                Object[] n = (Object[]) nodes[below(r, nodes.length)];
                Object[] m = (Object[]) n[2];
                sum += ((long[]) n[0])[1] + (m == null ? 0 : ((int[]) m[1])[2]);
                Object[] chunk = (Object[]) chunks[below(r >>> 21, chunks.length)];
                if (chunk != null) {
                    Object[] section = (Object[]) ((Object[]) chunk[0])[(int) ((r >>> 40) % 24)];
                    sum += ((long[]) section[0])[(int) ((r >>> 8) & 127)];
                }
            }
            // 临时对象：寻路节点、网络包、事件。
            long allocated = 0;
            Object keep = null;
            while (allocated < budget) {
                r = next(r);
                if ((r & 7) == 0) {
                    byte[] packet = new byte[4096];
                    packet[(int) (r & 4095)] = 1;
                    keep = packet;
                    allocated += 4112;
                } else {
                    Object[] node = new Object[] {new long[4], new int[] {(int) r, 1}, keep};
                    keep = node;
                    allocated += 32 + 48 + 24;
                }
                sum += allocated & 1;
            }
            sink += sum + (keep == null ? 0 : 1);
            long done = System.nanoTime();
            if (measuring && tickCount < ticks.length) {
                ticks[tickCount++] = done - started;
            }
            next += 50_000_000L;
            if (done - next > 1_000_000_000L) {
                next = done;
            }
        }
    }

    /** 渲染：不封顶，每帧读 512 个区块拼顶点缓冲。 */
    static void render() {
        long r = 0x7F4A7C159E3779B9L;
        long last = System.nanoTime();
        while (!stop) {
            long sum = 0;
            float[][] buffers = new float[16][];
            for (int b = 0; b < buffers.length; b++) {
                buffers[b] = new float[4096];
            }
            for (int c = 0; c < 512; c++) {
                r = next(r);
                Object[] chunk = (Object[]) chunks[below(r, chunks.length)];
                if (chunk == null) {
                    continue;
                }
                Object[] sections = (Object[]) chunk[0];
                float[] buf = buffers[c & 15];
                for (int s = 0; s < 24; s++) {
                    long[] states = (long[]) ((Object[]) sections[s])[0];
                    for (int i = 0; i < states.length; i++) {
                        long v = states[i];
                        int at = (i * 31 + s * 7) & 4095;
                        buf[at] += (float) ((v >>> 11) & 1023) * 0.001f;
                        sum += v & 1;
                    }
                }
            }
            for (int i = 0; i < 64; i++) {
                r = next(r);
                int[] tex = (int[]) textures[below(r, textures.length)];
                sum += tex[(int) ((r >>> 9) & 1023)];
            }
            for (float[] buf : buffers) {
                sum += (long) buf[(int) (r & 4095)];
            }
            sink += sum;
            long now = System.nanoTime();
            if (measuring && frameCount < frames.length) {
                frames[frameCount++] = now - last;
            }
            last = now;
        }
    }
}
